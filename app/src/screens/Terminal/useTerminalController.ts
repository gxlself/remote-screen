import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import type {
  RootScreenProps,
  RootStackParamList,
} from '../../app/navigation/routes';
import { useToast } from '../../components/Toast';
import { showToast } from '../../components/ToastHost';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { useSshServers } from '../../features/data/queries';
import { needsSessionAuth } from '../../features/session/auth';
import { selectLiveSsh, sshStore } from '../../features/sessions/sshStore';
import { useSetting } from '../../features/settings/store';
import { sshServerName } from '../../features/ssh/prompts';
import { snippetText } from '../../features/terminal/snippets';
import type { Snippet } from '../../features/terminal/snippets';
import {
  initTabs,
  matchTab,
  shouldSendPrefill,
  tabPhase,
  tabsReducer,
} from '../../features/terminal/tabs';
import type { TabsAction } from '../../features/terminal/tabs';
import { terminal } from '../../i18n/zh/terminal';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import type { TerminalHandle } from './TerminalSlot';

type HostKeyParams = RootStackParamList['HostKeyPrompt'];

export function useTerminalController({
  route,
  navigation,
}: RootScreenProps<'Terminal'>) {
  const core = useCore();
  const platform = usePlatform();
  const servers = useSshServers();
  const [tmux] = useSetting('terminal.tmux');
  const [fontSize] = useSetting('terminal.fontSize');
  const [beforeSession] = useSetting('lock.beforeSession');
  const [sensitiveHosts] = useSetting('lock.sensitiveHosts');
  // S03: the App lock that guards Host and adapter sessions guards the
  // terminal too, once on entry, before any SSH connection opens.
  const [unlocked, setUnlocked] = useState(
    () =>
      !needsSessionAuth(route.params.serverId, {
        beforeSession,
        sensitiveHosts,
      }),
  );
  const [closing, setClosing] = useState<string | null>(null);
  const [s, reduce] = useReducer(tabsReducer, undefined, () =>
    initTabs(route.params, selectLiveSsh(sshStore.getState())),
  );
  const latest = useRef(s);
  latest.current = s;
  const handleRef = useRef<TerminalHandle>(null);
  const started = useRef(new Map<string, symbol>());
  const explicitlyClosed = useRef(new Map<string, string | null>());
  const retiredIds = useRef(new Set<string>());
  const mounted = useRef(true);
  // A second first-connection prompt waits until the first one is answered.
  const promptOpen = useRef(false);
  const pendingPrompts = useRef<HostKeyParams[]>([]);
  const { toast, show, hide } = useToast();

  // Keep back-to-back events in the same JS turn in sync before React renders.
  const dispatch = useCallback((action: TabsAction) => {
    latest.current = tabsReducer(latest.current, action);
    reduce(action);
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // SSH outlives the screen; only an explicit tab close disconnects it.
    };
  }, []);

  const serversLoading = servers.isLoading;
  const serverName = sshServerName(servers.data, route.params.serverId);
  const authStarted = useRef(false);
  useEffect(() => {
    // Wait for the list so the prompt names the server; a failed list still
    // falls back to its id.
    if (unlocked || serversLoading || authStarted.current) {
      return;
    }
    authStarted.current = true;
    platform
      .authenticate(terminal.authenticate(serverName))
      .catch(() => 'failed' as const)
      .then(result => {
        if (!mounted.current) {
          return;
        }
        if (result === 'cancelled' || result === 'failed') {
          navigation.goBack();
        } else {
          setUnlocked(true);
        }
      });
  }, [navigation, platform, serverName, serversLoading, unlocked]);

  const showPrompt = useCallback(
    (params: HostKeyParams) => {
      promptOpen.current = true;
      navigation.navigate('HostKeyPrompt', params);
    },
    [navigation],
  );

  useEffect(
    () =>
      navigation.addListener('focus', () => {
        promptOpen.current = false;
        let next = pendingPrompts.current.shift();
        while (next !== undefined && retiredIds.current.has(next.sshId)) {
          next = pendingPrompts.current.shift();
        }
        if (next !== undefined) {
          showPrompt(next);
        }
      }),
    [navigation, showPrompt],
  );

  useEffect(
    () =>
      navigation.addListener('beforeRemove', () => {
        if (latest.current.tabs.some(tab => tabPhase(tab) === 'open')) {
          showToast({ message: terminal.inBackground });
        }
      }),
    [navigation],
  );

  useCoreEvent('ssh.state', event => {
    if (retiredIds.current.has(event.sshId)) {
      return;
    }
    const tab = matchTab(latest.current, event.sshId, event.serverId);
    if (tab === null) {
      return;
    }
    if (tab.sshId === null) {
      dispatch({ type: 'opened', key: tab.key, sshId: event.sshId });
    }
    dispatch({
      type: 'sshState',
      sshId: event.sshId,
      state: event.state,
      message: event.message,
    });
  });

  useCoreEvent('ssh.authUrl', event => {
    if (retiredIds.current.has(event.sshId)) {
      return;
    }
    const tab = matchTab(latest.current, event.sshId, event.serverId);
    if (tab === null) {
      return;
    }
    if (tab.sshId === null) {
      dispatch({ type: 'opened', key: tab.key, sshId: event.sshId });
    }
    dispatch({ type: 'authUrl', sshId: event.sshId });
    navigation.navigate('RootConfirm', {
      sshId: event.sshId,
      serverId: event.serverId,
      url: event.url,
      user: event.user,
    });
  });

  useCoreEvent('ssh.hostKeyPrompt', event => {
    if (retiredIds.current.has(event.sshId)) {
      return;
    }
    const tab = matchTab(latest.current, event.sshId, event.serverId);
    if (tab === null) {
      return;
    }
    if (tab.sshId === null) {
      dispatch({ type: 'opened', key: tab.key, sshId: event.sshId });
    }
    const params: HostKeyParams = {
      sshId: event.sshId,
      serverId: event.serverId,
      fp: event.fp,
      keyType: event.keyType,
      kind: event.kind,
      oldFp: event.oldFp,
    };
    if (promptOpen.current) {
      pendingPrompts.current.push(params);
    } else {
      showPrompt(params);
    }
  });

  useEffect(() => {
    if (!unlocked) {
      return;
    }
    for (const tab of s.tabs) {
      if (tabPhase(tab) !== 'opening' || started.current.has(tab.key)) {
        continue;
      }
      const attempt = Symbol(tab.key);
      started.current.set(tab.key, attempt);
      (async () => {
        try {
          const sshId = await core.ssh.open(tab.serverId, { tmux });
          if (explicitlyClosed.current.has(tab.key)) {
            if (explicitlyClosed.current.get(tab.key) !== sshId) {
              core.ssh.close(sshId);
            }
            return;
          }
          if (mounted.current && started.current.get(tab.key) === attempt) {
            dispatch({ type: 'opened', key: tab.key, sshId });
          }
        } catch (error) {
          if (mounted.current && started.current.get(tab.key) === attempt) {
            dispatch({
              type: 'openFailed',
              key: tab.key,
              message: error instanceof Error ? error.message : String(error),
            });
          }
        }
      })();
    }
  }, [core, dispatch, s.tabs, tmux, unlocked]);

  const activeTab = s.tabs.find(tab => tab.key === s.activeKey) ?? null;
  useEffect(() => {
    if (
      activeTab !== null &&
      shouldSendPrefill(activeTab) &&
      activeTab.prefill !== null &&
      handleRef.current !== null
    ) {
      handleRef.current.sendSequence(activeTab.prefill);
      dispatch({ type: 'prefillSent', key: activeTab.key });
    }
  }, [activeTab, dispatch]);

  const activeOpen = () => {
    const tab = latest.current.tabs.find(
      item => item.key === latest.current.activeKey,
    );
    return tab !== undefined && tabPhase(tab) === 'open';
  };

  const sendSnippet = (snippet: Snippet) => {
    if (activeOpen()) {
      handleRef.current?.sendSequence(snippetText(snippet));
    } else {
      show(terminal.notConnected);
    }
  };

  const paste = () => {
    if (activeOpen()) {
      handleRef.current?.paste();
    } else {
      show(terminal.notConnected);
    }
  };

  // The last tab closing leaves the page up with a way to open another.
  const closeTab = (key: string) => {
    const tab = latest.current.tabs.find(item => item.key === key);
    if (tab === undefined) {
      return;
    }
    explicitlyClosed.current.set(key, tab.sshId);
    if (tab.sshId !== null) {
      retiredIds.current.add(tab.sshId);
      core.ssh.close(tab.sshId);
    }
    dispatch({ type: 'close', key });
  };

  // T02: a connected tab asks first; one still connecting closes at once.
  const requestClose = (key: string) => {
    const tab = latest.current.tabs.find(item => item.key === key);
    if (tab === undefined) {
      return;
    }
    if (tabPhase(tab) === 'open') {
      setClosing(key);
    } else {
      closeTab(key);
    }
  };

  const confirmClose = () => {
    if (closing !== null) {
      closeTab(closing);
    }
    setClosing(null);
  };

  const reconnect = (key: string) => {
    const tab = latest.current.tabs.find(item => item.key === key);
    if (tab?.sshId) {
      retiredIds.current.add(tab.sshId);
    }
    started.current.delete(key);
    dispatch({ type: 'reopen', key });
  };

  return {
    s,
    activeTab,
    handleRef,
    tmux,
    fontSize,
    unlocked,
    toast,
    hideToast: hide,
    showCopied: () => show(terminal.copied),
    sendSnippet,
    paste,
    canSend: activeTab !== null && tabPhase(activeTab) === 'open',
    closing: s.tabs.find(tab => tab.key === closing) ?? null,
    requestClose,
    confirmClose,
    cancelClose: () => setClosing(null),
    reconnect,
    activate: (key: string) => dispatch({ type: 'activate', key }),
    openNew: (serverId: string) => dispatch({ type: 'open', serverId }),
  };
}
