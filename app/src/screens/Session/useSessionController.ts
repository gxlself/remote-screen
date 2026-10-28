import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AccessibilityInfo } from 'react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { useToast } from '../../components/Toast';
import type { ToastState } from '../../components/Toast';
import { showToast as showGlobalToast } from '../../components/ToastHost';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { useDirectory, useIdentity } from '../../features/data/queries';
import { needsSessionAuth } from '../../features/session/auth';
import { hudModel } from '../../features/session/hud';
import {
  rememberQuality,
  rememberedQuality,
} from '../../features/session/qualityMemory';
import { useSessions, useSessionsActions } from '../../features/session/store';
import {
  hostSwitcherItem,
  shortcutTarget,
} from '../../features/session/switcher';
import type {
  SwitcherItem,
  SwitcherList,
} from '../../features/session/switcher';
import {
  openComputerPicker,
  replaceWithSession,
  useElsewhereIds,
  useSwitcherList,
  withoutElsewhere,
} from '../../features/session/useSessionSwitcher';
import { useSpaces } from '../../features/session/useSpaces';
import type { SpacesController } from '../../features/session/useSpaces';
import { clearVideo } from '../../features/session/videoStore';
import {
  useWindowId,
  useWindowPage,
} from '../../features/session/WindowContext';
import {
  MAIN_WINDOW,
  focusOtherWindow,
  openInNewWindow,
  otherWindowShowing,
  windows,
} from '../../features/session/windows';
import { deriveSessionView, endRoute } from '../../features/session/view';
import { getRecordingUi } from '../../features/recording/sessionRecording';
import { useHostPrefs, useSetting } from '../../features/settings/store';
import { useKv } from '../../features/storage/StorageContext';
import { session } from '../../i18n/zh/session';
import { sessionHud } from '../../i18n/zh/sessionHud';
import { useKeyboardInset } from '../../native/keyboard/keyboardInset';
import { usePlatform } from '../../native/platform/PlatformContext';
import { maxFps } from '../../native/platform/impl/system';
import {
  activateWindow,
  windowsSupported,
} from '../../native/window/window';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { SessionShortcutEvent } from '../../native/keyboard/KeyCapture';
import type { RemoteSessionHandle } from '../../native/session/RemoteSessionView';
import type {
  OpenOpts,
  Quality,
  SessionId,
  SessionPhase,
} from '../../native/rscore/types';
import type { SessionCtx, SheetKind } from './slots';
import { ensureNotifyBeforeSession } from '../../features/session/notifyPermission';

export const TOOLBAR_HINT_KEY = 'session.toolbarHiddenHint';

// 外壳私有状态不进入 slots.ts；插槽始终只接收 SessionCtx。
export type SessionController = SessionCtx & {
  sheet: SheetKind | null;
  coachVisible: boolean;
  toast: ToastState | null;
  hideToast(): void;
  closeKeyboard(): void;
  keyboardInset: number;
  onGesture(e: { kind: string }): void;
  onFirstFrame(): void;
  /** 已连接时点「断开」先弹的确认；recording 表示确认框要说明录制会停止。 */
  disconnectPrompt: { recording: boolean } | null;
  confirmDisconnect(): void;
  dismissDisconnect(): void;
  showToolbar(): void;
  /** 首帧迟迟不来时关掉当前连接重新连一次。 */
  retryConnect(): void;
  /** 切换浮层列出的会话。 */
  switcherList: SwitcherList;
  /** 收起本会话（连接保持），全屏打开另一个会话。 */
  switchTo(item: SwitcherItem): void;
  /** 收起本会话，去首页选电脑。 */
  connectOther(): void;
  /** ⌃⌥⌘ 快捷键：上一个 / 下一个 / 第 N 个 / 打开浮层。 */
  onShortcut(event: SessionShortcutEvent): void;
  /** 三指左右滑 / 快捷键横滑切换电脑、位置小圆点、切过来时的快照。 */
  spaces: SpacesController;
  /** 在别的窗口里显示的会话。 */
  elsewhereIds: string[];
  /** （iPad）：在新窗口打开；不支持多窗口时没有。 */
  openNewWindow?: (item: SwitcherItem) => void;
  /** 本页在主窗口：当前会话也能移到新窗口。 */
  inMainWindow: boolean;
};

export function useSessionController({
  route,
  navigation,
}: RootScreenProps<'Session'>): SessionController {
  const {
    hostId,
    sessionId: requestedId,
    viewOnly,
    displayId,
    notice,
  } = route.params;
  const core = useCore();
  const platform = usePlatform();
  const kv = useKv();
  const windowId = useWindowId();
  // a session on screen in another window is not taken over here.
  const [elsewhere] = useState(() =>
    windowsSupported()
      ? otherWindowShowing(windows.getState(), windowId, requestedId)
      : null,
  );
  const { directory, isLoading } = useDirectory();
  const identity = useIdentity();
  const [phoneMode] = useSetting('session.mode.phone');
  const [tabletMode] = useSetting('session.mode.tablet');
  const [defaultQuality] = useSetting('quality.default');
  const [sound] = useSetting('session.sound');
  const [beforeSession] = useSetting('lock.beforeSession');
  const [sensitiveHosts] = useSetting('lock.sensitiveHosts');
  const [coachShown, setCoachShown] = useSetting('session.coachShown');
  const [{ lockOnEnd }] = useHostPrefs(hostId);
  const { dismiss } = useSessionsActions();
  const [isPad] = useState(() => platform.deviceInfo().isPad);
  const [mode, setMode] = useState(isPad ? tabletMode : phoneMode);
  const [quality, setLocalQuality] = useState(
    () => rememberedQuality(kv, hostId) ?? defaultQuality,
  );
  const [sessionId, setSessionId] = useState<SessionId | null>(null);
  const videoRef = useRef<RemoteSessionHandle | null>(null);
  const requested = useSessions(state =>
    requestedId === undefined ? undefined : state[requestedId],
  );
  const rec = useSessions(state =>
    sessionId === null ? undefined : state[sessionId],
  );
  const host = directory?.computers.find(item => item.hostId === hostId);
  const hostName = host?.name ?? rec?.hostName ?? requested?.hostName ?? hostId;
  const os = host?.os ?? 'macos';
  const myDeviceId = identity.data?.deviceId ?? null;
  const view = deriveSessionView(rec, {
    os,
    quality,
    myDeviceId,
    ...(host === undefined ? {} : { features: host.features }),
  });
  const [startedAtMs, setStartedAtMs] = useState(Date.now);
  const [phaseAtMs, setPhaseAtMs] = useState<
    Partial<Record<SessionPhase, number>>
  >({});
  const [firstFrameAtMs, setFirstFrameAtMs] = useState<number | null>(null);
  const [sheet, setSheet] = useState<SheetKind | null>(null);
  const [coachVisible, setCoachVisible] = useState(false);
  const [keyboardOpen, setKeyboardOpen] = useState(false);
  const keyboardInset = useKeyboardInset();
  const [toolbarHidden, setToolbarHidden] = useState(false);
  const [disconnectPrompt, setDisconnectPrompt] = useState<{
    recording: boolean;
  } | null>(null);
  const [reduceMotion, setReduceMotion] = useState(false);
  const { toast, show: showToast, hide: hideToast } = useToast();
  const mounted = useRef(false);
  const leaving = useRef(false);
  const initialStarted = useRef(false);
  const opening = useRef(false);
  const notifyChecked = useRef(false);
  const noticeShown = useRef(false);
  const handledEnds = useRef(new Set<SessionId>());
  const streamed = useRef(new Set<SessionId>());
  // 本页自己打开的会话；没出画面就离开时要关掉，别留在后台。
  const openedHere = useRef(new Set<SessionId>());
  const collapsed = useRef(false);
  const lastDisplay = useRef<{
    id: SessionId;
    displayId: number | null;
  } | null>(null);
  const warnedNoFirstFrame = useRef(new Set<SessionId>());
  const lastPhase = useRef<{ id: SessionId; phase: SessionPhase } | null>(null);
  const current = useRef({ sessionId, rec, core });
  current.current = { sessionId, rec, core };
  useEffect(() => {
    if (sessionId !== null) {
      return () => clearVideo(sessionId);
    }
  }, [sessionId]);
  // EventsProvider seeds its store in an effect; wait for that first effect pass.
  const [ready, setReady] = useState(false);

  useEffect(() => {
    mounted.current = true;
    setReady(true);
    const opened = openedHere.current;
    const seen = streamed.current;
    return () => {
      mounted.current = false;
      // 收起和出过画面的会话保留在 Core 里；本页打开、还没连上就离开的要关掉。
      const { sessionId: id, core: api } = current.current;
      if (
        id !== null &&
        opened.has(id) &&
        !seen.has(id) &&
        !leaving.current &&
        !collapsed.current
      ) {
        closeQuietly(api, id);
      }
    };
  }, []);

  useEffect(() => {
    let active = true;
    let motionChanged = false;
    const subscription = AccessibilityInfo.addEventListener(
      'reduceMotionChanged',
      value => {
        motionChanged = true;
        setReduceMotion(value);
      },
    );
    AccessibilityInfo.isReduceMotionEnabled()
      .then(value => {
        if (active && !motionChanged) {
          setReduceMotion(value);
        }
      })
      .catch(() => {});
    return () => {
      active = false;
      subscription.remove();
    };
  }, []);

  // A connection that fell back to Host says so once; re-renders and
  // reconnects of this screen stay quiet.
  useEffect(() => {
    if (notice === undefined || noticeShown.current) {
      return;
    }
    noticeShown.current = true;
    showToast(notice);
  }, [notice, showToast]);

  const openSession = useCallback(async () => {
    if (opening.current || leaving.current) {
      return;
    }
    opening.current = true;
    try {
      if (!notifyChecked.current) {
        notifyChecked.current = true;
        try {
          await ensureNotifyBeforeSession({ platform, kv });
        } catch {
          // Permission checks must never prevent opening the session.
        }
        // 系统权限框可能停留很久，用户可能已经取消或返回。
        if (!mounted.current || leaving.current) {
          return;
        }
      }
      const opts: OpenOpts = {
        intent: viewOnly ? 'view' : 'control',
        quality,
        mode,
        audio: sound,
        maxFps: maxFps(),
        ...(displayId === undefined ? {} : { displayId }),
      };
      const id = await core.sessions.open(hostId, opts);
      if (!mounted.current || leaving.current) {
        // 拨号期间已经取消或离开：这条会话没人要了。
        closeQuietly(core, id);
        return;
      }
      openedHere.current.add(id);
      setFirstFrameAtMs(null);
      setPhaseAtMs({});
      lastPhase.current = null;
      setSessionId(id);
    } catch (error) {
      if (mounted.current && !leaving.current) {
        leaving.current = true;
        navigation.replace('SessionEnded', {
          hostId,
          hostName,
          detail: {
            message: error instanceof Error ? error.message : String(error),
          },
        });
      }
    } finally {
      opening.current = false;
    }
  }, [
    core,
    displayId,
    hostId,
    hostName,
    kv,
    mode,
    navigation,
    platform,
    quality,
    sound,
    viewOnly,
  ]);

  useEffect(() => {
    if (!ready || initialStarted.current) {
      return;
    }
    if (elsewhere !== null) {
      initialStarted.current = true;
      leaving.current = true;
      activateWindow(elsewhere);
      navigation.goBack();
      return;
    }
    if (requested !== undefined && requested.phase !== 'ended') {
      initialStarted.current = true;
      setSessionId(requested.id);
      return;
    }
    // 等目录查询完成，以主机名显示身份验证理由；查询失败仍可拨号。
    if (isLoading) {
      return;
    }
    initialStarted.current = true;
    const start = async () => {
      if (
        requestedId === undefined &&
        needsSessionAuth(hostId, {
          beforeSession,
          sensitiveHosts,
        })
      ) {
        let result;
        try {
          result = await platform.authenticate(session.authenticate(hostName));
        } catch {
          result = 'failed';
        }
        if (!mounted.current || leaving.current) {
          return;
        }
        if (result === 'cancelled' || result === 'failed') {
          leaving.current = true;
          navigation.goBack();
          return;
        }
        // P9 未接入时结果是 unavailable，默认放行。
      }
      if (mounted.current && !leaving.current) {
        await openSession();
      }
    };
    start();
  }, [
    beforeSession,
    elsewhere,
    hostId,
    hostName,
    isLoading,
    navigation,
    openSession,
    platform,
    ready,
    requested,
    requestedId,
    sensitiveHosts,
  ]);

  const onFirstFrame = useCallback(() => {
    if (sessionId !== null) {
      setFirstFrameAtMs(value => value ?? Date.now());
    }
  }, [sessionId]);

  const recordPhase = useCallback((id: SessionId, next: SessionPhase) => {
    if (lastPhase.current?.id === id && lastPhase.current.phase === next) {
      return;
    }
    lastPhase.current = { id, phase: next };
    const atMs = Date.now();
    setPhaseAtMs(times => ({ ...times, [next]: atMs }));
  }, []);

  useCoreEvent('session.state', event => {
    if (event.sessionId === current.current.sessionId) {
      recordPhase(event.sessionId, event.phase);
    }
  });

  const phase = rec?.phase;
  useEffect(() => {
    if (phase !== undefined && sessionId !== null) {
      recordPhase(sessionId, phase);
    }
  }, [phase, recordPhase, sessionId]);

  useEffect(() => {
    if (
      sessionId === null ||
      phase !== 'streaming' ||
      firstFrameAtMs !== null ||
      warnedNoFirstFrame.current.has(sessionId)
    ) {
      return;
    }
    const timer = setTimeout(() => {
      warnedNoFirstFrame.current.add(sessionId);
      core.logs.write('warn', 'session', 'no first frame 8s');
    }, 8000);
    return () => clearTimeout(timer);
  }, [core, firstFrameAtMs, phase, sessionId]);

  useEffect(() => {
    if (
      sessionId === null ||
      phase !== 'streaming' ||
      streamed.current.has(sessionId)
    ) {
      return;
    }
    streamed.current.add(sessionId);
    if (lockOnEnd) {
      core.sessions.action(sessionId, 'lockOnEnd', true);
    }
    if (!coachShown) {
      setCoachVisible(true);
    }
  }, [coachShown, core, lockOnEnd, phase, sessionId]);

  useEffect(() => {
    if (
      rec === undefined ||
      handledEnds.current.has(rec.id) ||
      leaving.current
    ) {
      return;
    }
    const end = endRoute(rec);
    if (rec.phase === 'ended') {
      clearVideo(rec.id);
    }
    if (end.kind === 'stay') {
      return;
    }
    handledEnds.current.add(rec.id);
    leaving.current = true;
    const leave = () => {
      dismiss(rec.id);
      if (end.kind === 'goBack') {
        navigation.goBack();
      } else {
        navigation.replace('SessionEnded', {
          hostId,
          hostName,
          sessionId: rec.id,
          ...(end.code === undefined ? {} : { code: end.code }),
          ...(end.reason === undefined ? {} : { reason: end.reason }),
          ...(end.detail === undefined ? {} : { detail: end.detail }),
        });
      }
    };
    if (end.kind === 'goBack' && end.toast !== undefined) {
      // 全局 Toast 跟着回到上一页，不用在黑屏上干等两秒。
      showGlobalToast({ message: end.toast });
    }
    leave();
  }, [dismiss, hostId, hostName, navigation, rec]);

  useEffect(() => {
    if (sessionId === null || rec === undefined) {
      return;
    }
    const previous = lastDisplay.current;
    lastDisplay.current = { id: sessionId, displayId: rec.displayId };
    if (
      previous === null ||
      previous.id !== sessionId ||
      previous.displayId === null ||
      rec.displayId === null ||
      previous.displayId === rec.displayId
    ) {
      return;
    }
    const index = rec.displays.findIndex(item => item.id === rec.displayId);
    if (index >= 0) {
      showToast(session.display.switched(index + 1), { tone: 'good' });
    }
  }, [rec, sessionId, showToast]);

  const endNow = () => {
    setDisconnectPrompt(null);
    if (sessionId === null) {
      leaving.current = true;
      navigation.goBack();
    } else {
      clearVideo(sessionId);
      core.sessions.close(sessionId);
    }
  };

  const disconnect = () => {
    const connected =
      sessionId !== null &&
      rec?.phase === 'streaming' &&
      firstFrameAtMs !== null &&
      rec.idleWarning === null;
    if (!connected) {
      // 连接中、重连中或闲置倒计时里的「断开」本身就是取消，直接结束。
      endNow();
      return;
    }
    const recordingUi = getRecordingUi();
    setSheet(null);
    setDisconnectPrompt({
      recording:
        recordingUi.sessionId === sessionId &&
        (recordingUi.state === 'starting' ||
          recordingUi.state === 'recording' ||
          recordingUi.state === 'stopping'),
    });
  };

  const setQuality = (next: Quality) => {
    if (next === quality) {
      return;
    }
    const previous = quality;
    setLocalQuality(next);
    if (sessionId !== null) {
      try {
        core.sessions.setQuality(sessionId, next);
      } catch {
        setLocalQuality(previous);
        showToast(
          session.quality.failed(sessionHud.toolbar.quality.labels[previous]),
          {
            tone: 'bad',
            action: {
              label: session.quality.retry,
              onPress: () => setQuality(next),
            },
          },
        );
        return;
      }
      showToast(
        session.quality.changed(sessionHud.toolbar.quality.labels[next]),
      );
    }
    rememberQuality(kv, hostId, next);
  };

  const retryConnect = () => {
    const old = sessionId;
    if (old !== null) {
      // 旧会话是我们主动关的，结束事件不能把页面带走。
      handledEnds.current.add(old);
      clearVideo(old);
      closeQuietly(core, old);
      dismiss(old);
    }
    setSessionId(null);
    setStartedAtMs(Date.now());
    setFirstFrameAtMs(null);
    setPhaseAtMs({});
    lastPhase.current = null;
    openSession();
  };

  const switcherList = useSwitcherList(null);
  // other windows see which session this page shows.
  useWindowPage(
    sessionId !== null && rec !== undefined
      ? hostSwitcherItem(rec, hostName)
      : null,
  );
  const elsewhereIds = useElsewhereIds();
  const spaceList = useMemo(
    () => withoutElsewhere(switcherList, elsewhereIds),
    [elsewhereIds, switcherList],
  );
  // Like 收起: the session stays in Core and on the session tab.
  const leaveCollapsed = () => {
    collapsed.current = true;
    if (sessionId !== null) {
      clearVideo(sessionId);
    }
  };
  const switchTo = (item: SwitcherItem) => {
    setSheet(null);
    if (item.id === sessionId || focusOtherWindow(item.id, windowId)) {
      return;
    }
    leaveCollapsed();
    replaceWithSession(navigation, item);
  };
  const connectOther = () => {
    setSheet(null);
    leaveCollapsed();
    openComputerPicker(navigation);
  };
  const openNewWindow = (item: SwitcherItem) => {
    setSheet(null);
    if (openInNewWindow(item, windowId) === 'moving') {
      // The window opens once this page has folded the session away.
      leaveCollapsed();
      openComputerPicker(navigation);
    }
  };
  const spaces = useSpaces({
    list: spaceList,
    currentId: sessionId,
    routeSessionId: requestedId,
    framed: firstFrameAtMs !== null,
    videoRef,
    switchTo,
  });
  const onShortcut = (event: SessionShortcutEvent) => {
    if (event.action === 'switcher') {
      setSheet('switcher');
      return;
    }
    // ⌃⌥⌘N counts as the switcher numbers; ←/→ skip other windows' ones.
    const target = shortcutTarget(
      event.action === 'index' ? switcherList.screens : spaceList.screens,
      sessionId,
      event,
    );
    if (target !== null) {
      // The same slide as three fingers, toward where the target sits.
      spaces.slideTo(target);
    }
  };

  const toggleToolbar = () => {
    const hide = !toolbarHidden;
    setToolbarHidden(hide);
    if (hide && kv.getString(TOOLBAR_HINT_KEY) !== '1') {
      kv.set(TOOLBAR_HINT_KEY, '1');
      showToast(session.toolbar.hiddenHint);
    }
  };

  return {
    hostId,
    hostName,
    host,
    os,
    myDeviceId,
    sessionId,
    videoRef,
    rec,
    view,
    hud: hudModel(rec),
    isPad,
    reduceMotion,
    mode,
    setMode,
    quality,
    setQuality,
    startedAtMs,
    phaseAtMs,
    firstFrameAtMs,
    openSheet: setSheet,
    closeSheet: () => setSheet(null),
    openCoach: () => setCoachVisible(true),
    closeCoach: () => {
      setCoachVisible(false);
      setCoachShown(true);
    },
    keyboardOpen,
    toggleKeyboard: () => setKeyboardOpen(value => !value),
    toolbarHidden,
    collapse: () => {
      collapsed.current = true;
      if (sessionId !== null) {
        clearVideo(sessionId);
      }
      // popTo: navigate would push a second MainTabs over this page.
      navigation.popTo('MainTabs', { screen: 'Sessions' });
    },
    disconnect,
    cancelConnect: endNow,
    openTerminal: () => {
      if (host !== undefined) {
        if (sessionId !== null) {
          clearVideo(sessionId);
        }
        navigation.navigate('Terminal', { serverId: host.node });
      }
    },
    showToast,
    sheet,
    coachVisible,
    toast,
    hideToast,
    keyboardInset,
    closeKeyboard: () => setKeyboardOpen(false),
    onGesture: ({ kind }) => {
      if (kind === 'threeFingerTap') {
        toggleToolbar();
      } else if (kind === 'twoFingerDoubleTap') {
        setMode(value => (value === 'trackpad' ? 'touch' : 'trackpad'));
      }
    },
    onFirstFrame,
    disconnectPrompt,
    confirmDisconnect: endNow,
    dismissDisconnect: () => setDisconnectPrompt(null),
    showToolbar: () => setToolbarHidden(false),
    retryConnect,
    switcherList,
    switchTo,
    connectOther,
    onShortcut,
    spaces,
    elsewhereIds,
    ...(windowsSupported() ? { openNewWindow } : {}),
    inMainWindow: windowId === MAIN_WINDOW,
  };
}

function closeQuietly(core: RsCoreApi, id: SessionId): void {
  try {
    clearVideo(id);
    core.sessions.close(id);
  } catch {
    // 会话可能已经在 Core 里结束；关不掉也不影响离开页面。
  }
}
