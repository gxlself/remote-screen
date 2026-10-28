import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from 'react';
import { AppState, Keyboard, Platform } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { showToast } from '../../components/ToastHost';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import type { Server, SshServer } from '../../native/rscore/types';
import {
  selectCollapsed,
  useAdapterLive,
  type CollapsedAdapter,
} from '../../features/adapters/liveStore';
import { adaptersAvailable } from '../../features/adapters/logic';
import { MIN_HOST_VERSION } from '../../features/diagnostics/hostVersion';
import {
  cardKey,
  cardModel,
  entryReach,
  nativeCardModel,
  sortCards,
  type CardModel,
  type HomeCard,
} from '../../features/directory/cardModel';
import { pickPrewarm, usePrewarm } from '../../features/directory/prewarm';
import {
  entrySearchFields,
  filterServers,
  matchesQuery,
} from '../../features/directory/search';
import { useHostPath } from '../../features/directory/pathStore';
import type { FallbackGuideState } from '../../features/fallback/useRemoteFallback';
import { useManualReachControl } from '../../features/native/reach';
import type { ComputerEntry } from '../../features/native/types';
import { useComputerEntries } from '../../features/native/useComputerEntries';
import { decideRoute, useConnect } from '../../features/native/useConnect';
import {
  selectLive,
  useSessions,
  type SessionRecord,
} from '../../features/session/store';
import { judgeNetwork } from '../../features/network/judge';
import { tailnetInUse } from '../../features/network/tailnetUse';
import { useNetwork } from '../../features/network/store';
import { useManualServers } from '../../features/servers/useManualServers';
import { DEFAULT_HOST_PREFS } from '../../features/settings/schema';
import { useSetting } from '../../features/settings/store';
import { useKv } from '../../features/storage/StorageContext';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { useEmbeddedSwitch } from '../../features/tailnet/useEmbeddedSwitch';
import { useTailscaleApi } from '../../features/tailscale/useTailscaleApi';
import { useDirectory, useSshServers } from '../../features/data/queries';
import { home } from '../../i18n/zh/home';
import type { LiveBarSession } from './LiveSessionBar';
import type { ScrollViewHandle } from './useTabScrollToTop';
import { openFirst, tailscaleUrls } from '../../features/platform/externalApps';

export type HomeStatusTone = 'good' | 'warn' | 'unknown';

export type HomeStatus = {
  tone: HomeStatusTone;
  text: string;
};

export type HomeBanner = {
  id: 'vpnConflict' | 'tailnetDown' | 'embeddedLogin' | 'embeddedBoth';
  tone: 'warn' | 'good';
  title: string;
  message?: string;
  announce?: boolean;
  action?: {
    label: string;
    onPress: () => void;
    loading?: boolean;
  };
  secondary?: { label: string; onPress: () => void };
  /** Hides the banner for good. */
  onDismiss?: () => void;
  /** The embedded connection needs a Headscale address before switching. */
  controlUrlSheet?: { onClose(): void; onDone(): void };
  onUseEmbedded?: () => void;
};

export type ServerItem = {
  serverId: string;
  name: string;
  subtitle: string;
  needsTailnet: boolean;
  /** A tailnet machine no server covers yet; tapping adds it. */
  suggestion?: { host: string; online: boolean };
};

export type HomeActions = {
  openCard(card: HomeCard): void;
  longPressCard(card: HomeCard): void;
  openServer(server: ServerItem): void;
  openTerminal(hostId: string): void;
  add(): void;
  /** Pairing / Host install, the empty list's secondary link. */
  installHost(): void;
  openSettings(): void;
  backToSession(): void;
  /** Opens the Tailscale app, from the empty list while it is down. */
  openTailscale?(): void;
};

export type HomeViewProps = {
  headerStatus: HomeStatus;
  banners: HomeBanner[];
  cards: HomeCard[];
  servers: ServerItem[];
  /** The floating bar: the latest live Host or folded-away adapter session. */
  liveSession: LiveBarSession | null;
  isLoading: boolean;
  isEmpty: boolean;
  /** The directory could not be read and nothing is listed. */
  loadError: Error | null;
  onRefresh: () => void;
  refreshing: boolean;
  actions: HomeActions;
  /** The card whose connection is starting; the others wait for it. */
  busyKey?: string | null;
  /** Why the list is empty, when it is not just "no computers yet". */
  emptyReason?: 'tailnetDown';
};

export type HomeScreenViewProps = HomeViewProps & {
  query: string;
  setQuery(q: string): void;
  /** Scrolled to the top when the current tab is tapped again. */
  scrollRef?: RefObject<ScrollViewHandle | null>;
};

/** The view props plus the connection guide, which the screen renders once. */
export type HomeModel = HomeViewProps & {
  guide: FallbackGuideState;
  closeGuide(): void;
};

type HomeNavigation = TabScreenProps<'Home'>['navigation'];
type OptionalNavigation = HomeNavigation | undefined;
type HostEntry = Extract<ComputerEntry, { kind: 'host' }>;

const SSH_PORT = 22;
/** Card states that read "minutes ago" are recomputed this often. */
const CLOCK_TICK_MS = 60_000;
export const EMBEDDED_BOTH_DISMISSED_KEY = 'home.embeddedBothDismissed';

function authLabel(server: SshServer): string {
  if (server.tailscaleSsh === true || server.auth === 'tailscale') {
    return home.servers.tailscaleSsh;
  }
  return server.auth === 'password'
    ? home.servers.passwordAuth
    : home.servers.keyAuth;
}

function serverSubtitle(server: SshServer): string {
  if (server.source === 'manual' && server.jump !== null) {
    return home.servers.viaJump(server.jump);
  }
  return home.servers.subtitle(server.os ?? '', authLabel(server));
}

function asDirectoryServer(server: SshServer): Server {
  return {
    node: server.name,
    addr: server.host,
    online: true,
    os: server.os ?? '',
    tailscaleSsh: server.tailscaleSsh,
  };
}

function latestLiveSession(sessions: SessionRecord[]): SessionRecord | null {
  return sessions.at(-1) ?? null;
}

function adapterBarSession(record: CollapsedAdapter): LiveBarSession {
  return {
    id: record.sessionId,
    hostId: record.params.target.hostId ?? `adapter:${record.sessionId}`,
    hostName: record.name,
    startedAt: record.startedAtUs,
    path: null,
    adapter: { phase: record.phase, params: record.params },
  };
}

function useMinuteClock(): number {
  const [nowUs, setNowUs] = useState(() => Date.now() * 1000);
  useEffect(() => {
    const timer = setInterval(() => setNowUs(Date.now() * 1000), CLOCK_TICK_MS);
    return () => clearInterval(timer);
  }, []);
  return nowUs;
}

function appActive(): boolean {
  const state = AppState.currentState;
  return state !== 'inactive' && state !== 'background';
}

/** Manual computers are probed only while Home is focused and in the foreground. */
function useHomeVisible(navigation: OptionalNavigation): boolean {
  const [focused, setFocused] = useState(() => navigation?.isFocused() ?? true);
  const [active, setActive] = useState(appActive);

  useEffect(() => {
    if (navigation === undefined) {
      return;
    }
    setFocused(navigation.isFocused());
    const offFocus = navigation.addListener('focus', () => setFocused(true));
    const offBlur = navigation.addListener('blur', () => setFocused(false));
    return () => {
      offFocus();
      offBlur();
    };
  }, [navigation]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', state =>
      setActive(state === 'active'),
    );
    return () => subscription.remove();
  }, []);

  return focused && active;
}

export function useHomeModel(
  query: string,
  navigation?: OptionalNavigation,
): HomeModel {
  const platform = usePlatform();
  const core = useCore();
  const kv = useKv();
  const sw = useEmbeddedSwitch();
  const [urlSheet, setUrlSheet] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [loggingIn, setLoggingIn] = useState(false);
  const [bothDismissed, setBothDismissed] = useState(
    () => kv.getString(EMBEDDED_BOTH_DISMISSED_KEY) === '1',
  );
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const busyKeyRef = useRef<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const refreshingRef = useRef(false);
  const isAndroid = Platform.OS === 'android';
  const directoryQuery = useDirectory();
  const sshQuery = useSshServers();
  const manualServers = useManualServers();
  const entries = useComputerEntries();
  const visible = useHomeVisible(navigation);
  const { reach: manualReach, refresh: refreshReach } = useManualReachControl(
    entries,
    visible,
  );
  const { connect, guide, closeGuide } = useConnect();
  const [connectPrefer] = useSetting('connect.prefer');
  const [hostPrefs] = useSetting('hosts.prefs');
  const [networks] = useSetting('networks');
  const [controlUrl] = useSetting('tailnet.controlUrl');
  const tailnetEnabled = useTailnetEnabled();
  const tailscaleApi = useTailscaleApi();
  const networkStatus = useNetwork(state => state.status);
  const liveSessions = useSessions(selectLive);
  const collapsedAdapters = useAdapterLive(selectCollapsed);
  const nowUs = useMinuteClock();

  const directory = directoryQuery.directory;
  const computers = useMemo(() => directory?.computers ?? [], [directory]);
  const hostEntries = useMemo(
    () =>
      new Map(
        entries.flatMap((entry): [string, HostEntry][] =>
          entry.kind === 'host' ? [[entry.computer.hostId, entry]] : [],
        ),
      ),
    [entries],
  );
  const sshServers = useMemo(() => sshQuery.data ?? [], [sshQuery.data]);
  const verdict = judgeNetwork(networkStatus, networks);
  const latestSession = latestLiveSession(liveSessions);
  // The session record keeps the id it was opened with; show the list's name.
  const liveName =
    latestSession === null
      ? undefined
      : hostEntries.get(latestSession.hostId)?.computer.name;
  const hostLive = useMemo(
    () =>
      latestSession === null || liveName === undefined
        ? latestSession
        : { ...latestSession, hostName: liveName },
    [latestSession, liveName],
  );
  // V33: a folded-away Screen Sharing / Remote Desktop session shares the
  // bar; the one started last wins.
  const latestAdapter = collapsedAdapters.at(-1) ?? null;
  const liveSession = useMemo<LiveBarSession | null>(
    () =>
      latestAdapter === null ||
      (hostLive !== null && hostLive.startedAt > latestAdapter.startedAtUs)
        ? hostLive
        : adapterBarSession(latestAdapter),
    [hostLive, latestAdapter],
  );
  const liveByHost = useMemo(
    () => new Map(liveSessions.map(session => [session.hostId, session])),
    [liveSessions],
  );
  const liveHostId = hostLive?.hostId ?? '';
  const livePath = useHostPath(liveHostId);

  // Every card, before the search; pre-warming does not follow the query.
  const allCards = useMemo(
    () =>
      entries.map((entry): { entry: ComputerEntry; card: HomeCard } => {
        if (entry.kind !== 'host') {
          return {
            entry,
            card: nativeCardModel(entry, entryReach(entry, manualReach)),
          };
        }
        const { computer } = entry;
        return {
          entry,
          card: cardModel({
            computer,
            nowUs,
            hostPath:
              computer.hostId === hostLive?.hostId ? livePath : undefined,
            liveSession: liveByHost.get(computer.hostId),
            verdict,
            minHostVersion: MIN_HOST_VERSION,
            tailnetEnabled,
          }),
        };
      }),
    [
      entries,
      liveByHost,
      hostLive?.hostId,
      livePath,
      manualReach,
      nowUs,
      tailnetEnabled,
      verdict,
    ],
  );

  const cards = useMemo(
    () =>
      sortCards(
        allCards
          .filter(({ entry }) => matchesQuery(query, entrySearchFields(entry)))
          .map(({ card }) => card),
      ),
    [allCards, query],
  );

  const prewarmHostIds = useMemo(
    () =>
      pickPrewarm(
        sortCards(
          allCards
            .map(({ card }) => card)
            .filter((card): card is CardModel => card.kind !== 'native'),
        ),
      ),
    [allCards],
  );
  usePrewarm(prewarmHostIds);

  const filteredSshServers = useMemo(() => {
    const matching = new Set(
      filterServers(sshServers.map(asDirectoryServer), query).map(
        server => server.node,
      ),
    );
    return sshServers.filter(server => matching.has(server.name));
  }, [query, sshServers]);

  // core.ssh.servers() lists only manual servers, so tailnet machines from
  // the directory that none of them covers are suggested.
  const suggestedServers = useMemo<Server[]>(() => {
    if (sshQuery.data === undefined || !tailnetEnabled) {
      return [];
    }
    const hosts = new Set<string>();
    const names = new Set<string>();
    for (const server of sshQuery.data) {
      hosts.add(server.host.toLowerCase());
      names.add(server.name.toLowerCase());
      names.add(server.serverId.toLowerCase());
    }
    for (const server of manualServers.servers) {
      hosts.add(server.host.toLowerCase());
      names.add(server.name.toLowerCase());
      names.add(server.id.toLowerCase());
    }
    const next: Server[] = [];
    for (const server of directory?.servers ?? []) {
      const node = server.node.toLowerCase();
      if (
        server.addr === '' ||
        hosts.has(server.addr.toLowerCase()) ||
        names.has(node)
      ) {
        continue;
      }
      names.add(node);
      next.push(server);
    }
    return next;
  }, [directory, manualServers.servers, sshQuery.data, tailnetEnabled]);

  const servers = useMemo<ServerItem[]>(
    () => [
      ...filteredSshServers.map(server => ({
        serverId: server.serverId,
        name: server.name,
        subtitle: serverSubtitle(server),
        needsTailnet:
          tailnetEnabled &&
          verdict.tailnet === 'down' &&
          server.source === 'directory',
      })),
      ...filterServers(suggestedServers, query).map(server => ({
        serverId: `suggest:${server.node}`,
        name: server.node,
        subtitle: home.servers.suggestion(server.addr),
        needsTailnet: false,
        suggestion: { host: server.addr, online: server.online },
      })),
    ],
    [filteredSshServers, query, suggestedServers, tailnetEnabled, verdict.tailnet],
  );

  const online = useMemo(
    () =>
      entries.filter(entry =>
        entry.kind === 'host'
          ? entry.computer.online
          : entryReach(entry, manualReach) === 'online',
      ).length,
    [entries, manualReach],
  );

  // Someone who never set up Tailscale is not told that it is down.
  const usesTailnet = useMemo(
    () =>
      tailnetInUse({
        enabled: tailnetEnabled,
        entries,
        sshServers,
        manualServers: manualServers.servers,
        apiConfigured: tailscaleApi.status?.configured === true,
        embeddedConfigured:
          sw.available === true && controlUrl != null && controlUrl !== '',
      }),
    [
      controlUrl,
      entries,
      manualServers.servers,
      sshServers,
      sw.available,
      tailnetEnabled,
      tailscaleApi.status?.configured,
    ],
  );

  const headerStatus = useMemo<HomeStatus>(() => {
    const count = online;
    if (!tailnetEnabled) {
      return { tone: 'unknown', text: home.status.online(count) };
    }
    if (verdict.tailnet === 'up') {
      if (verdict.via === 'embedded') {
        return {
          tone: 'good',
          text: home.status.tailnetOnlineEmbedded(count),
        };
      }
      return { tone: 'good', text: home.status.tailnetOnline(count) };
    }
    if (
      verdict.tailnet === 'down' &&
      (usesTailnet || verdict.companyVpn !== null)
    ) {
      return {
        tone: 'warn',
        text:
          verdict.companyVpn === null
            ? home.status.tailnetDown
            : home.status.tailnetDownCompanyVpn,
      };
    }
    return { tone: 'unknown', text: home.status.online(count) };
  }, [
    online,
    tailnetEnabled,
    usesTailnet,
    verdict.companyVpn,
    verdict.tailnet,
    verdict.via,
  ]);

  const openTailscale = useCallback(() => {
    openFirst(platform, tailscaleUrls()).catch(() => undefined);
  }, [platform]);

  const useEmbedded = useCallback(() => {
    if (switching) {
      return;
    }
    setSwitching(true);
    sw.switchToEmbedded().then(
      result => {
        setSwitching(false);
        if (result === 'unavailable') {
          showToast({ message: home.embedded.switchFailed, tone: 'bad' });
        }
        setUrlSheet(result === 'needsControlUrl');
      },
      () => {
        setSwitching(false);
        showToast({ message: home.embedded.switchFailed, tone: 'bad' });
      },
    );
  }, [sw, switching]);

  const openLogin = useCallback(() => {
    if (loggingIn) {
      return;
    }
    setLoggingIn(true);
    sw.openLogin().then(
      () => setLoggingIn(false),
      () => {
        setLoggingIn(false);
        showToast({ message: home.embedded.loginFailed, tone: 'bad' });
      },
    );
  }, [loggingIn, sw]);

  const dismissBoth = useCallback(() => {
    try {
      kv.set(EMBEDDED_BOTH_DISMISSED_KEY, '1');
    } catch {
      // Still hidden for this run.
    }
    setBothDismissed(true);
  }, [kv]);

  const isLoading = directoryQuery.isLoading && directory === undefined;
  const isEmpty =
    !isLoading &&
    directory !== undefined &&
    !manualServers.isLoading &&
    entries.length === 0 &&
    sshServers.length === 0 &&
    suggestedServers.length === 0;
  const emptyReason =
    isEmpty && usesTailnet && verdict.tailnet === 'down'
      ? 'tailnetDown'
      : undefined;

  const banners = useMemo<HomeBanner[]>(() => {
    const next: HomeBanner[] = [];
    // Every banner here is about Tailscale; switched off, there are none.
    if (!tailnetEnabled) {
      return next;
    }
    if (verdict.banner === 'vpnConflict') {
      if (sw.available !== true) {
        next.push({
          id: 'vpnConflict',
          tone: 'warn',
          announce: false,
          title: home.banners.vpnConflictTitle,
          message: isAndroid
            ? home.androidVpn.message
            : home.banners.vpnConflictMessage,
        });
      } else {
        next.push({
          id: 'vpnConflict',
          tone: 'warn',
          announce: false,
          title: home.banners.vpnConflictTitle,
          message: isAndroid
            ? home.embedded.conflictMessageAndroid
            : home.embedded.conflictMessageIos,
          action: {
            label: home.embedded.useEmbedded,
            onPress: useEmbedded,
            loading: switching,
          },
          ...(isAndroid ? { onUseEmbedded: useEmbedded } : {}),
          ...(urlSheet
            ? {
                controlUrlSheet: {
                  onClose: () => setUrlSheet(false),
                  onDone: () => setUrlSheet(false),
                },
              }
            : {}),
        });
      }
    } else if (verdict.banner === 'embeddedLogin') {
      next.push({
        id: 'embeddedLogin',
        tone: 'warn',
        announce: false,
        title: home.embedded.loginTitle,
        message: home.embedded.loginMessage,
        action: {
          label: home.embedded.login,
          onPress: openLogin,
          loading: loggingIn,
        },
      });
    } else if (verdict.banner === 'tailnetDown') {
      // The empty list says the same with its own button.
      if (usesTailnet && emptyReason !== 'tailnetDown') {
        next.push({
          id: 'tailnetDown',
          tone: 'warn',
          announce: false,
          title: home.banners.tailnetDownTitle,
          action: {
            label: home.banners.openTailscale,
            onPress: openTailscale,
          },
        });
      }
    } else if (
      verdict.via === 'embedded' &&
      verdict.tailnet === 'up' &&
      verdict.tailnetAddr !== null &&
      verdict.companyVpn !== null &&
      sw.canUndo &&
      !bothDismissed
    ) {
      next.push({
        id: 'embeddedBoth',
        tone: 'good',
        announce: false,
        title: home.embedded.bothTitle,
        message: home.embedded.bothMessage,
        action: {
          label: home.embedded.settings,
          onPress: () =>
            navigation?.navigate('MainTabs', { screen: 'Settings' }),
        },
        onDismiss: dismissBoth,
        ...(isAndroid
          ? { secondary: { label: home.embedded.undo, onPress: sw.undo } }
          : {}),
      });
    }
    return next;
  }, [
    bothDismissed,
    dismissBoth,
    emptyReason,
    openLogin,
    openTailscale,
    isAndroid,
    loggingIn,
    navigation,
    sw,
    switching,
    tailnetEnabled,
    urlSheet,
    useEmbedded,
    usesTailnet,
    verdict.banner,
    verdict.companyVpn,
    verdict.tailnet,
    verdict.tailnetAddr,
    verdict.via,
  ]);

  const opensHostSession = useCallback(
    (entry: HostEntry) =>
      decideRoute({
        entry,
        adaptersAvailable: adaptersAvailable(core),
        connectPrefer,
        hostPrefs: {
          ...DEFAULT_HOST_PREFS,
          ...hostPrefs[entry.computer.hostId],
        },
        discoverAvailable: core.discover !== undefined,
      }).kind === 'session',
    [connectPrefer, core, hostPrefs],
  );

  /** One connection at a time: the tapped card spins, the others wait. */
  const connectCard = useCallback(
    (card: HomeCard, entry: ComputerEntry) => {
      const key = cardKey(card);
      busyKeyRef.current = key;
      setBusyKey(key);
      platform.haptic('light');
      connect(entry)
        .catch(() => undefined)
        .finally(() => {
          if (busyKeyRef.current === key) {
            busyKeyRef.current = null;
            setBusyKey(null);
          }
        });
    },
    [connect, platform],
  );

  const openCard = useCallback(
    (card: HomeCard) => {
      if (busyKeyRef.current !== null) {
        return;
      }
      if (card.kind === 'native') {
        connectCard(card, card.entry);
        return;
      }
      const live = liveByHost.get(card.hostId);
      if (live !== undefined) {
        navigation?.navigate('Session', {
          hostId: card.hostId,
          sessionId: live.id,
        });
        return;
      }
      const entry = hostEntries.get(card.hostId);
      if (entry === undefined) {
        return;
      }
      // An outdated Host only blocks the Host session. Screen Sharing / Remote
      // Desktop works without it, so the native-first route still probes.
      if (card.tap.kind === 'error' && opensHostSession(entry)) {
        navigation?.navigate('SessionEnded', {
          hostId: card.hostId,
          hostName: card.name,
          code: card.tap.code,
        });
        return;
      }
      connectCard(card, entry);
    },
    [connectCard, hostEntries, liveByHost, navigation, opensHostSession],
  );

  const longPressCard = useCallback(
    (card: HomeCard) => {
      Keyboard.dismiss();
      platform.haptic('light');
      if (card.kind === 'native') {
        navigation?.navigate('NativeSheet', { entryKey: card.key });
        return;
      }
      navigation?.navigate('DeviceSheet', { hostId: card.hostId });
    },
    [navigation, platform],
  );

  const openServer = useCallback(
    (server: ServerItem) => {
      if (server.suggestion !== undefined) {
        navigation?.navigate('AddServer', {
          prefill: {
            name: server.name,
            host: server.suggestion.host,
            port: SSH_PORT,
          },
        });
        return;
      }
      navigation?.navigate('Terminal', { serverId: server.serverId });
    },
    [navigation],
  );

  const openTerminal = useCallback(
    (hostId: string) => {
      const computer = computers.find(candidate => candidate.hostId === hostId);
      if (computer === undefined) {
        return;
      }
      navigation?.navigate('Terminal', { serverId: computer.node });
    },
    [computers, navigation],
  );

  const add = useCallback(() => {
    navigation?.navigate('AddComputer');
  }, [navigation]);

  const installHost = useCallback(() => {
    navigation?.navigate('PairAdd');
  }, [navigation]);

  const openSettings = useCallback(() => {
    navigation?.navigate('MainTabs', { screen: 'Settings' });
  }, [navigation]);

  const backToSession = useCallback(() => {
    if (liveSession === null) {
      return;
    }
    if (liveSession.adapter !== undefined) {
      navigation?.navigate('AdapterSession', {
        ...liveSession.adapter.params,
        sessionId: liveSession.id,
      });
      return;
    }
    navigation?.navigate('Session', {
      hostId: liveSession.hostId,
      sessionId: liveSession.id,
    });
  }, [liveSession, navigation]);

  const actions = useMemo<HomeActions>(
    () => ({
      openCard,
      longPressCard,
      openServer,
      openTerminal,
      add,
      installHost,
      openSettings,
      backToSession,
      openTailscale,
    }),
    [
      add,
      backToSession,
      installHost,
      longPressCard,
      openCard,
      openServer,
      openSettings,
      openTailscale,
      openTerminal,
    ],
  );

  const refetchDirectory = directoryQuery.refetch;
  const refetchServers = sshQuery.refetch;
  // The spinner is this pull's own: it waits for every list it reloads and
  // does not appear for background refetches.
  const onRefresh = useCallback(() => {
    if (refreshingRef.current) {
      return;
    }
    refreshingRef.current = true;
    setRefreshing(true);
    Promise.allSettled([
      refetchDirectory(),
      refetchServers(),
      refreshReach(),
    ]).finally(() => {
      refreshingRef.current = false;
      setRefreshing(false);
    });
  }, [refetchDirectory, refetchServers, refreshReach]);

  return {
    headerStatus,
    banners,
    cards,
    servers,
    liveSession,
    isLoading,
    isEmpty,
    loadError: directoryQuery.error ?? null,
    onRefresh,
    refreshing,
    actions,
    busyKey,
    emptyReason,
    guide,
    closeGuide,
  };
}
