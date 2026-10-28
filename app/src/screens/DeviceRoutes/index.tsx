import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  RefreshControl,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Switch } from '../../components/Switch';
import { Icon } from '../../components/icons/Icon';
import { useCore } from '../../native/rscore/CoreContext';
import type { Computer, NetPath, ProbeId } from '../../native/rscore/types';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { EarlyProbeEvents } from '../../features/events/earlyProbeEvents';
import { useDirectory } from '../../features/data/queries';
import { buildRouteRows, summaryText } from '../../features/directory/routes';
import { useHostPath, useHostRoutes } from '../../features/directory/pathStore';
import { judgeNetwork, localHostAddrs } from '../../features/network/judge';
import { useNetwork } from '../../features/network/store';
import { selectLive, useSessions } from '../../features/session/store';
import { useHostPrefs, useSetting } from '../../features/settings/store';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { useEmbeddedTailnet } from '../../features/tailnet/useEmbeddedTailnet';
import { deviceRoutes } from '../../i18n/zh/deviceRoutes';
import { radius, space, tokens } from '../../theme/tokens';
import type { RootScreenProps } from '../../app/navigation/routes';
import PolicyPicker from './PolicyPicker';
import RouteRow from './RouteRow';

const PROBE_TIMEOUT_MS = 5000;
const EMPTY_ADDRESSES: Computer['addresses'] = [];
const EMPTY_DETAIL = { node: '', addresses: EMPTY_ADDRESSES };

function currentPath(
  path: ReturnType<typeof useHostPath>,
  record: ReturnType<typeof selectLive>[number] | undefined,
): NetPath | null {
  return record?.path ?? path?.path ?? null;
}

export default function DeviceRoutesScreen({
  navigation,
  route,
}: RootScreenProps<'DeviceRoutes'>) {
  const { hostId } = route.params;
  const core = useCore();
  const insets = useSafeAreaInsets();
  const directory = useDirectory();
  const computer = directory.directory?.computers.find(
    item => item.hostId === hostId,
  );
  const networks = useSetting('networks')[0];
  const [prefs, setPrefs] = useHostPrefs(hostId);
  const routes = useHostRoutes(hostId);
  const hostPath = useHostPath(hostId);
  const liveSession = useSessions(state =>
    selectLive(state).find(record => record.hostId === hostId),
  );
  const network = useNetwork(state => state.status);
  const embedded = useEmbeddedTailnet({ pollMs: 10_000 });
  const tailnetEnabled = useTailnetEnabled();
  const [testing, setTesting] = useState(false);
  const [probeFailed, setProbeFailed] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const probeId = useRef<ProbeId | null>(null);
  const probeGeneration = useRef(0);
  const timeout = useRef<ReturnType<typeof setTimeout> | null>(null);
  const wasTesting = useRef(false);
  const earlyEventsRef = useRef(new EarlyProbeEvents());

  const finishProbe = useCallback(() => {
    probeGeneration.current += 1;
    if (timeout.current !== null) {
      clearTimeout(timeout.current);
      timeout.current = null;
    }
    probeId.current = null;
    earlyEventsRef.current.clear();
    setTesting(false);
    setRefreshing(false);
  }, []);

  const startProbe = useCallback(async () => {
    const generation = probeGeneration.current + 1;
    probeGeneration.current = generation;
    if (timeout.current !== null) {
      clearTimeout(timeout.current);
    }
    probeId.current = null;
    earlyEventsRef.current.clear();
    setTesting(true);
    setProbeFailed(false);
    timeout.current = setTimeout(() => {
      if (probeGeneration.current === generation) {
        finishProbe();
      }
    }, PROBE_TIMEOUT_MS);
    try {
      const nextId = await core.diag.probe(hostId);
      if (probeGeneration.current === generation) {
        probeId.current = nextId;
        for (const event of earlyEventsRef.current.take(nextId)) {
          if (probeGeneration.current !== generation) {
            break;
          }
          if (event.last) {
            finishProbe();
            break;
          }
        }
      }
    } catch {
      if (probeGeneration.current === generation) {
        finishProbe();
        setProbeFailed(true);
      }
    }
  }, [core, finishProbe, hostId]);

  useCoreEvent('probe.result', event => {
    if (event.hostId !== hostId) {
      return;
    }
    if (probeId.current === null) {
      earlyEventsRef.current.push(event);
      return;
    }
    if (event.probeId !== probeId.current) {
      return;
    }
    if (event.last) {
      finishProbe();
    }
  });

  useEffect(() => {
    const earlyEvents = earlyEventsRef.current;
    let active = true;
    let needsProbe = !navigation.isFocused();
    if (!needsProbe) {
      startProbe().catch(() => undefined);
    }
    const unsubscribe = navigation.addListener('focus', () => {
      if (active && needsProbe) {
        needsProbe = false;
        startProbe().catch(() => undefined);
      }
    });
    const unsubscribeBlur = navigation.addListener('blur', () => {
      needsProbe = true;
    });
    return () => {
      active = false;
      unsubscribe();
      unsubscribeBlur();
      if (timeout.current !== null) {
        clearTimeout(timeout.current);
        timeout.current = null;
      }
      earlyEvents.clear();
    };
  }, [navigation, startProbe]);

  useEffect(
    () => () => {
      probeGeneration.current += 1;
    },
    [],
  );

  const localAddrs = useMemo(() => localHostAddrs(network), [network]);
  const verdict = useMemo(
    () => judgeNetwork(network, networks),
    [network, networks],
  );
  // With Tailscale off its addresses are not listed (nor named).
  const items = useMemo(
    () =>
      buildRouteRows({
        detail: computer ?? EMPTY_DETAIL,
        verdict,
        networks,
        localAddrs,
        routes,
        inUsePath: currentPath(hostPath, liveSession),
        derpRegion:
          hostPath?.derpRegion ?? liveSession?.derpRegion ?? undefined,
        testing,
        hasLiveSession: liveSession !== undefined,
        embedded: {
          active: embedded.route?.active === 'embedded',
          status: embedded.status,
        },
      }).filter(row => tailnetEnabled || row.kind !== 'tailnet'),
    [
      computer,
      verdict,
      networks,
      localAddrs,
      routes,
      hostPath,
      liveSession,
      testing,
      embedded.route?.active,
      embedded.status,
      tailnetEnabled,
    ],
  );
  const name = computer?.name ?? '';

  useEffect(() => {
    if (Platform.OS !== 'ios') {
      wasTesting.current = testing;
      return;
    }
    if (wasTesting.current && !testing && !probeFailed) {
      AccessibilityInfo.announceForAccessibility(summaryText(items, false));
    }
    wasTesting.current = testing;
  }, [items, probeFailed, testing]);

  useEffect(() => {
    if (probeFailed && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(deviceRoutes.probeFailed);
    }
  }, [probeFailed]);

  const onRefresh = useCallback(() => {
    setRefreshing(true);
    startProbe().catch(() => undefined);
  }, [startProbe]);

  return (
    <View style={[styles.screen, { paddingTop: insets.top }]}>
      <View style={styles.header}>
        <IconButton
          accessibilityLabel={deviceRoutes.a11y.back}
          icon="chevronLeft"
          onPress={() => navigation.goBack()}
          size={44}
          variant="raised"
        />
        <AppText
          accessibilityRole="header"
          style={styles.title}
          variant="headline"
          weight={600}
        >
          {name}
        </AppText>
        <View style={styles.backSpacer} />
      </View>

      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: Math.max(insets.bottom, space.gutter) + space.lg },
        ]}
        refreshControl={
          <RefreshControl
            onRefresh={onRefresh}
            refreshing={refreshing}
            tintColor={tokens.text2}
          />
        }
      >
        {probeFailed ? (
          <View
            accessibilityLiveRegion="polite"
            accessibilityRole="alert"
            style={styles.error}
            testID="probe-error"
          >
            <Icon color="bad" name="alert" size={18} />
            <AppText color="bad" variant="footnote">
              {deviceRoutes.probeFailed}
            </AppText>
          </View>
        ) : null}

        <View
          accessible
          accessibilityActions={[
            { name: 'retest', label: deviceRoutes.a11y.retest },
          ]}
          accessibilityLabel={deviceRoutes.a11y.addressHeader(
            summaryText(items, testing),
          )}
          accessibilityLiveRegion="polite"
          accessibilityRole="header"
          onAccessibilityAction={event => {
            if (event.nativeEvent.actionName === 'retest') {
              startProbe().catch(() => undefined);
            }
          }}
        >
          <SectionHeader
            right={summaryText(items, testing)}
            title={deviceRoutes.addressSection}
          />
        </View>
        <ListGroup>
          {items.map(item => (
            <RouteRow key={item.key} row={item} />
          ))}
          <ListRow
            accessibilityLabel={deviceRoutes.addAddress}
            left={<Icon color="accent" name="plus" size={18} />}
            onPress={() => navigation.navigate('AddByAddress')}
            title={deviceRoutes.addAddress}
            testID="add-address"
          />
        </ListGroup>
        <AppText color="text3" style={styles.explanation} variant="footnote">
          {deviceRoutes.explanation}
        </AppText>

        <SectionHeader title={deviceRoutes.policySection} />
        <PolicyPicker
          onChange={value => setPrefs({ routePolicy: value })}
          rows={items}
          tailnetEnabled={tailnetEnabled}
          value={prefs.routePolicy}
        />

        <ListGroup>
          <ListRow
            right={
              <Switch
                accessibilityLabel={deviceRoutes.switches.tcpFallback.title}
                onValueChange={value => setPrefs({ tcpFallback: value })}
                testID="tcp-fallback"
                value={prefs.tcpFallback}
              />
            }
            subtitle={deviceRoutes.switches.tcpFallback.subtitle}
            title={deviceRoutes.switches.tcpFallback.title}
          />
          <ListRow
            right={
              <Switch
                accessibilityLabel={deviceRoutes.switches.autoUpdateAddr.title}
                onValueChange={value => setPrefs({ autoUpdateAddr: value })}
                testID="auto-update-addr"
                value={prefs.autoUpdateAddr}
              />
            }
            subtitle={
              tailnetEnabled
                ? deviceRoutes.switches.autoUpdateAddr.subtitle
                : deviceRoutes.switches.autoUpdateAddr.subtitleLan
            }
            title={deviceRoutes.switches.autoUpdateAddr.title}
          />
        </ListGroup>
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  header: {
    minHeight: 52,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: space.gutter,
    gap: space.md,
  },
  backSpacer: {
    width: 44,
    height: 44,
  },
  title: {
    flex: 1,
    textAlign: 'center',
  },
  content: {
    paddingHorizontal: space.gutter,
    gap: space.md,
  },
  explanation: {
    paddingHorizontal: space.xs,
    lineHeight: 19,
  },
  error: {
    minHeight: 48,
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
    paddingHorizontal: space.md,
    borderRadius: radius.button,
    borderWidth: 1,
    borderColor: 'rgba(255,136,115,0.3)',
    backgroundColor: 'rgba(255,136,115,0.1)',
  },
});
