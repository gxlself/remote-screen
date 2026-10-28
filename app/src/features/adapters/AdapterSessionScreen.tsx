import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { JSX } from 'react';
import {
  ActivityIndicator,
  BackHandler,
  PixelRatio,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import type { LayoutChangeEvent } from 'react-native';
import type {
  RootScreenProps,
  RootStackParamList,
} from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { Icon } from '../../components/icons/Icon';
import { Toast, useToast } from '../../components/Toast';
import { adapters } from '../../i18n/zh/adapters';
import { useKeyboardInset } from '../../native/keyboard/keyboardInset';
import type { SessionShortcutEvent } from '../../native/keyboard/KeyCapture';
import { SessionKeyboard } from '../../native/keyboard/SessionKeyboard';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { PlatformServices } from '../../native/platform/PlatformServices';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type {
  AdapterContext,
  AdapterErrorInfo,
  AdapterOpenOpts,
  AdapterTarget,
  SavedCredentialInfo,
  SessionCombo,
  SessionId,
} from '../../native/rscore/types';
import { RemoteSessionView } from '../../native/session/RemoteSessionView';
import {
  activateWindow,
  windowsSupported,
} from '../../native/window/window';
import type { RemoteSessionHandle } from '../../native/session/RemoteSessionView';
import GestureCoach from '../../screens/Session/GestureCoach';
import PointerLockChip from '../../screens/Session/PointerLockChip';
import type { SessionCtx } from '../../screens/Session/slots';
import { useReduceMotion } from '../../screens/SessionEnded/useReduceMotion';
import { hit, radius, tokens } from '../../theme/tokens';
import { FallbackGuide } from '../fallback/FallbackGuide';
import { useRemoteFallback } from '../fallback/useRemoteFallback';
import { errorCodeLabel } from '../format/misc';
import { useDirectory } from '../data/queries';
import { needsSessionAuth } from '../session/auth';
import SessionSwitcher from '../session/SessionSwitcher';
import SpaceIndicator from '../session/SpaceIndicator';
import { adapterSwitcherStatus, shortcutTarget } from '../session/switcher';
import type { SwitcherItem } from '../session/switcher';
import {
  replaceWithSession,
  useElsewhereIds,
  useSwitcherList,
  withoutElsewhere,
} from '../session/useSessionSwitcher';
import { useAdapterPageLive } from '../session/useLiveSessionCount';
import { useSpaces } from '../session/useSpaces';
import {
  NARROW_WINDOW,
  useWindowId,
  useWindowPage,
  useWindowSize,
} from '../session/WindowContext';
import {
  MAIN_WINDOW,
  focusOtherWindow,
  openInNewWindow,
  otherWindowShowing,
  windows,
} from '../session/windows';
import { useAdapterHistory } from '../sessions/useAdapterHistory';
import type { AdapterHistoryTracker } from '../sessions/useAdapterHistory';
import { useSetting } from '../settings/store';
import { useTailnetEnabled } from '../tailnet/enabled';
import { AdapterError } from './AdapterError';
import { AdapterMoreSheet } from './AdapterMoreSheet';
import { AdapterToolbar } from './AdapterToolbar';
import { CertPrompt } from './CertPrompt';
import { CredentialSheet } from './CredentialSheet';
import {
  adapterLive,
  findCollapsed,
  resumeKind,
  routeParamsOf,
  showAdapterSession,
  shownElsewhere,
} from './liveStore';
import { useMonitorCount, type MonitorCount } from './monitorMemory';
import { WHOLE_DESKTOP } from './ScreenPicker';
import {
  adaptersAvailable,
  fallbackTargetFor,
  isEncryptedPath,
  type AdapterUiPhase,
  type CredInput,
} from './logic';
import { useToolbarDock } from './toolbarDock';
import { useAdapterAudio } from './useAdapterAudio';
import { useAdapterReconnect } from './useAdapterReconnect';
import { useAdapterSession } from './useAdapterSession';
import type { AdapterResume } from './useAdapterSession';

const RESIZE_DEBOUNCE_MS = 500;
/** Some decoders never report a first frame; stop waiting for it after this. */
export const FIRST_FRAME_FALLBACK_MS = 8000;
/** The waiting layer shows how long it has waited from this point on. */
const SHOW_WAITED_AFTER_S = 3;
/** How long the RDP takeover note stays after connecting (V26). */
export const TAKEOVER_NOTE_MS = 10000;
const NOT_SAVED: SavedCredentialInfo = { saved: false };
/** Above this the picture counts as zoomed in (更多 › 适应屏幕). */
const ZOOMED_IN = 1.01;
const UNAVAILABLE: AdapterErrorInfo = {
  code: 'io',
  rsCode: ADAPTER_RS_E.broken,
  message: 'adapter bindings unavailable',
};
const HOST_READY_CONTEXTS: readonly AdapterContext[] = [
  'loginWindow',
  'noHost',
  'fallback',
];

type Params = RootStackParamList['AdapterSession'];
type InputMode = 'trackpad' | 'touch';
type Confirm = 'disconnect' | 'forget' | null;
type CertFlow = 'none' | 'asked' | 'answered';

function readSaved(
  core: RsCoreApi,
  target: AdapterTarget,
): SavedCredentialInfo {
  try {
    return core.adapters?.saved(target) ?? NOT_SAVED;
  } catch {
    return NOT_SAVED;
  }
}

function readCanSave(core: RsCoreApi): boolean {
  try {
    return core.adapters?.canSave() ?? false;
  } catch {
    return false;
  }
}

function readIsPad(platform: PlatformServices): boolean {
  try {
    return platform.deviceInfo().isPad;
  } catch {
    return false;
  }
}

function toggleMode(mode: InputMode): InputMode {
  return mode === 'trackpad' ? 'touch' : 'trackpad';
}

/** Seconds since `active` last turned on; 0 while off. */
function useWaitedSeconds(active: boolean): number {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!active) {
      return;
    }
    setSeconds(0);
    const started = Date.now();
    const timer = setInterval(() => {
      setSeconds(Math.floor((Date.now() - started) / 1000));
    }, 1000);
    return () => clearInterval(timer);
  }, [active]);
  return active ? seconds : 0;
}

type BodyProps = {
  params: Params;
  displayName: string;
  navigation: RootScreenProps<'AdapterSession'>['navigation'];
  /** A folded-away session to pick up instead of connecting (V33). */
  resumeId: SessionId | null;
  notify(message: string): void;
  onLeave(): void;
  /** Leaves the page with the session still connected. */
  onCollapse(): void;
  /**
   * the page is about to be replaced by another session; false
   * when it is already leaving.
   */
  onSwitchAway(): boolean;
  onReopen(): void;
  onOtherApp(): void;
  onCopyCode(rsCode: number): void;
};

function AdapterSessionBody({
  params,
  displayName,
  navigation,
  resumeId,
  notify,
  onLeave,
  onCollapse,
  onSwitchAway,
  onReopen,
  onOtherApp,
  onCopyCode,
}: BodyProps): JSX.Element {
  const { target, addrKind, context } = params;
  const core = useCore();
  const platform = usePlatform();
  const dims = useWindowSize();
  const windowId = useWindowId();
  const [autoSync] = useSetting('clipboard.autoSync');
  const [cursorSpeed] = useSetting('session.cursorSpeed');
  const [naturalScroll] = useSetting('session.naturalScroll');
  const [haptics] = useSetting('session.haptics');
  const [phoneMode] = useSetting('session.mode.phone');
  const [tabletMode] = useSetting('session.mode.tablet');
  const [coachShown, setCoachShown] = useSetting('session.coachShown');
  const [sound, setSound] = useSetting('session.sound');
  const reduceMotion = useReduceMotion();
  const inset = useKeyboardInset();
  const { directory } = useDirectory();
  const [baseOpts] = useState<AdapterOpenOpts>(() => ({
    viewWidth: dims.width,
    viewHeight: dims.height,
    scale: PixelRatio.get(),
    useSaved: true,
    ...(context === undefined ? {} : { context }),
  }));
  // remembered per computer; a retry opens with the latest count.
  const [monitors, setMonitors] = useMonitorCount(target);
  const opts = useMemo<AdapterOpenOpts>(
    () =>
      target.protocol === 'rdp' ? { ...baseOpts, monitors } : baseOpts,
    [baseOpts, monitors, target.protocol],
  );
  // The count just asked for, until the new layout (or a reconnect) lands.
  const [monitorChange, setMonitorChange] = useState<MonitorCount | null>(
    null,
  );
  const [canSave] = useState(() => readCanSave(core));
  const [isPad] = useState(() => readIsPad(platform));
  // a narrow split-view window gets the phone toolbar.
  const padLayout = isPad && dims.width >= NARROW_WINDOW;
  const [resumed] = useState(() =>
    resumeId === null ? undefined : adapterLive.peek(resumeId),
  );
  // Still connected, or ended for a reason to show: picked up as it was. A
  // drop or a timeout while folded away connects again from scratch.
  const [resume] = useState<AdapterResume | undefined>(() =>
    resumed === undefined || resumeKind(resumed) === 'reopen'
      ? undefined
      : {
          sessionId: resumed.sessionId,
          phase: resumed.phase,
          error: resumed.error,
          width: resumed.width,
          height: resumed.height,
          serverName: resumed.serverName,
          screens: resumed.screens,
        },
  );
  const resumedId = resume?.phase === 'streaming' ? resume.sessionId : null;
  // V24: the default from Settings, per device class.
  const [mode, setMode] = useState<InputMode>(
    () => resumed?.mode ?? (isPad ? tabletMode : phoneMode),
  );
  const [keyboardOpen, setKeyboardOpen] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const collapsedBeforeKeyboard = useRef<boolean | null>(null);
  const [dock, setDock] = useToolbarDock();
  const [moreOpen, setMoreOpen] = useState(false);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  // Before the first picture the page counts from when it opened.
  const [openedAtUs] = useState(() => Date.now() * 1000);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [coachVisible, setCoachVisible] = useState(false);
  const [remoteClipboard, setRemoteClipboard] = useState<string | null>(null);
  const clipboardNoticeShown = useRef(false);
  const [lastCreds, setLastCreds] = useState<{
    user?: string;
    domain?: string;
  }>({});
  const [certFlow, setCertFlow] = useState<CertFlow>('none');
  const [hostReadyDismissed, setHostReadyDismissed] = useState(false);
  const videoRef = useRef<RemoteSessionHandle>(null);
  const [zoomed, setZoomed] = useState(false);
  const [page] = useState(() => Symbol('adapter-page'));

  const onClipboard = useCallback(
    (text: string) => {
      if (!autoSync) {
        // V40: kept for 更多 › 复制电脑上的剪贴板.
        setRemoteClipboard(text);
        return;
      }
      platform.copyText(text).catch(() => undefined);
      if (!clipboardNoticeShown.current) {
        clipboardNoticeShown.current = true;
        notify(adapters.toast.clipboardReceived);
      }
    },
    [autoSync, notify, platform],
  );
  // V33: leaving a live picture folds the session away instead of closing it.
  const tracker = useRef<AdapterHistoryTracker | null>(null);
  const handOff = useRef<(id: SessionId) => boolean>(() => false);
  const keepOnUnmount = useCallback((id: SessionId) => handOff.current(id), []);
  const session = useAdapterSession({
    target,
    opts,
    onClipboard,
    keepOnUnmount,
    ...(resume === undefined ? {} : { resume }),
  });
  const { state, screens, close, resize } = session;
  const phase = state.phase;
  const sessionId = state.sessionId;
  const streaming = phase === 'streaming';
  useAdapterAudio(sessionId, target.protocol, streaming, () =>
    notify(adapters.toast.soundFailed),
  );
  const adapterHistory = useAdapterHistory({
    target,
    fallbackName: displayName,
    sessionId,
    streaming,
    error: state.error,
    ...(resumed === undefined ? {} : { resume: resumed.history }),
  });
  tracker.current = adapterHistory;
  useEffect(
    () =>
      sessionId === null ? undefined : showAdapterSession(sessionId, page),
    [page, sessionId],
  );
  useEffect(() => {
    if (resumed === undefined) {
      return;
    }
    adapterLive.take(resumed.sessionId);
    // Back in front: the picture was paused while folded away.
    if (resumed.phase !== 'ended') {
      try {
        core.sessions.background(resumed.sessionId, false);
      } catch {
        core.logs.write('warn', 'adapters', 'resume picture failed');
      }
    }
  }, [core, resumed]);
  const reconnect = useAdapterReconnect({
    phase,
    error: state.error,
    retry: () => session.retry(),
    end: session.end,
    isAlive: session.isAlive,
  });
  // Keeps the screen awake and the app alive in the background meanwhile.
  useAdapterPageLive(sessionId, phase !== 'ended' || reconnect.active);

  // The last picture stays under the reconnect overlay (V01).
  const [lastStreamingId, setLastStreamingId] = useState<SessionId | null>(
    resumedId,
  );
  if (streaming && sessionId !== null && lastStreamingId !== sessionId) {
    setLastStreamingId(sessionId);
  }

  // V12: keep the placeholder until the first decoded frame.
  // A picked-up session already showed its picture.
  const [framedId, setFramedId] = useState<SessionId | null>(resumedId);
  const awaitingFrame =
    streaming && sessionId !== null && framedId !== sessionId;
  const latestSessionId = useRef(sessionId);
  latestSessionId.current = sessionId;
  // this page's own first picture (a picked-up session counts
  // as framed at once, but its surface still starts empty).
  const [pictureShown, setPictureShown] = useState(false);
  const onFirstFrame = useCallback(() => {
    setFramedId(latestSessionId.current);
    setPictureShown(true);
  }, []);
  useEffect(() => {
    if (!awaitingFrame) {
      return;
    }
    const timer = setTimeout(() => {
      core.logs.write('warn', 'adapters', 'first frame did not arrive');
      setFramedId(sessionId);
    }, FIRST_FRAME_FALLBACK_MS);
    return () => clearTimeout(timer);
  }, [awaitingFrame, core, sessionId]);
  const framed = streaming && !awaitingFrame;

  // Index into the latest adapter.screens, or WHOLE_DESKTOP. When a new
  // layout drops the selected screen, the native side falls back to 0 on its
  // own (C13.7); the whole desktop stays selected.
  const [screenIndex, setScreenIndex] = useState(
    () => (resume === undefined ? undefined : resumed?.screenIndex) ?? 0,
  );
  const [screenGone, setScreenGone] = useState(false);
  if (
    screenIndex !== 0 &&
    screenIndex !== WHOLE_DESKTOP &&
    screenIndex >= screens.length
  ) {
    setScreenIndex(0);
    setScreenGone(true);
  }
  useEffect(() => {
    if (screenGone) {
      setScreenGone(false);
      notify(adapters.toast.screenGone);
    }
  }, [notify, screenGone]);
  useEffect(() => setMonitorChange(null), [screens]);
  if (monitorChange !== null && phase === 'ended') {
    setMonitorChange(null);
  }
  const primedScreens = useRef<SessionId | null>(resume?.sessionId ?? null);
  // The first layout with several screens shows the primary one (C13.6).
  useEffect(() => {
    const api = core.adapters;
    if (
      sessionId === null ||
      api === undefined ||
      screens.length < 2 ||
      primedScreens.current === sessionId
    ) {
      return;
    }
    primedScreens.current = sessionId;
    try {
      api.selectScreen(sessionId, 0);
    } catch {
      core.logs.write('warn', 'adapters', 'select screen failed');
    }
  }, [core, screens, sessionId]);

  handOff.current = (id: SessionId): boolean => {
    const history = tracker.current;
    if (
      state.phase !== 'streaming' ||
      state.sessionId !== id ||
      history === null
    ) {
      return false;
    }
    // Another page picked this session up before this one went: it is in
    // front, so neither fold it away nor pause its picture.
    if (shownElsewhere(id, page)) {
      core.logs.write('info', 'adapters', 'session shown by another page');
      return true;
    }
    const nowUs = Date.now() * 1000;
    adapterLive.put({
      sessionId: id,
      params: routeParamsOf(params),
      name: history.name,
      phase: 'streaming',
      error: null,
      width: state.width,
      height: state.height,
      serverName: state.serverName,
      screens,
      screenIndex,
      mode,
      startedAtUs: history.live?.startedAtUs ?? nowUs,
      collapsedAtUs: nowUs,
      history: { ...history },
    });
    // The record carries the 最近 entry on; this unmount must not write it.
    history.live = null;
    // Folded away: no picture to decode until it comes back (b).
    try {
      core.sessions.background(id, true);
    } catch {
      core.logs.write('warn', 'adapters', 'pause picture failed');
    }
    return true;
  };

  // Saved credentials prefill the sheet and gate the toolbar; re-read them
  // whenever login is asked for or succeeds (a save lands on success).
  const [saved, setSaved] = useState<{
    phase: AdapterUiPhase;
    info: SavedCredentialInfo;
  }>(() => ({
    phase,
    // A picked-up session starts out streaming.
    info: phase === 'streaming' ? readSaved(core, target) : NOT_SAVED,
  }));
  if (saved.phase !== phase) {
    setSaved({
      phase,
      info:
        phase === 'needsCredentials' || phase === 'streaming'
          ? readSaved(core, target)
          : saved.info,
    });
  }
  if (state.cert !== null && certFlow === 'none') {
    setCertFlow('asked');
  }

  const closeKeyboard = useCallback(() => {
    setKeyboardOpen(false);
    const before = collapsedBeforeKeyboard.current;
    collapsedBeforeKeyboard.current = null;
    if (before !== null) {
      setCollapsed(before);
    }
  }, []);
  // V23: the toolbar steps aside while the keyboard is up.
  const openKeyboard = () => {
    collapsedBeforeKeyboard.current = collapsed;
    setCollapsed(true);
    setKeyboardOpen(true);
  };

  useEffect(() => {
    if (phase !== 'streaming') {
      closeKeyboard();
    }
  }, [closeKeyboard, phase]);

  // First picture: gesture help once (V61), the RDP takeover note (V26).
  const [takeoverNote, setTakeoverNote] = useState(false);
  const firstStream = useRef(resumed !== undefined);
  useEffect(() => {
    if (!framed || firstStream.current) {
      return;
    }
    firstStream.current = true;
    if (!coachShown) {
      setCoachVisible(true);
    }
    if (target.protocol === 'rdp') {
      setTakeoverNote(true);
    }
  }, [coachShown, framed, target.protocol]);
  useEffect(() => {
    if (!takeoverNote) {
      return;
    }
    const timer = setTimeout(() => setTakeoverNote(false), TAKEOVER_NOTE_MS);
    return () => clearTimeout(timer);
  }, [takeoverNote]);

  const cleanEnd =
    phase === 'ended' &&
    (state.error === null || state.error.code === 'cancelled');
  useEffect(() => {
    if (cleanEnd) {
      onLeave();
    }
  }, [cleanEnd, onLeave]);

  const disconnect = useCallback(() => {
    close();
    onLeave();
  }, [close, onLeave]);

  const closeCoach = useCallback(() => {
    setCoachVisible(false);
    setCoachShown(true);
  }, [setCoachShown]);

  const waiting =
    !reconnect.active &&
    (phase === 'opening' ||
      phase === 'connecting' ||
      phase === 'authenticating' ||
      phase === 'needsCredentials' ||
      awaitingFrame);
  const error =
    phase === 'ended' && !cleanEnd && !reconnect.active ? state.error : null;

  // V02: keyboard, then toolbar, then ask; overlays close first.
  const back = () => {
    if (confirm !== null) {
      setConfirm(null);
    } else if (switcherOpen) {
      setSwitcherOpen(false);
    } else if (moreOpen) {
      setMoreOpen(false);
    } else if (coachVisible) {
      closeCoach();
    } else if (reconnect.active) {
      reconnect.cancel();
    } else if (error !== null || phase === 'ended') {
      onLeave();
    } else if (!framed) {
      disconnect();
    } else if (keyboardOpen) {
      closeKeyboard();
    } else if (!collapsed) {
      setCollapsed(true);
    } else {
      // V33: the last step folds the session away; 断开 still asks first.
      onCollapse();
    }
  };
  const latestBack = useRef(back);
  latestBack.current = back;
  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }
    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        latestBack.current();
        return true;
      },
    );
    return () => subscription.remove();
  }, []);

  // A picked-up session may come back to another size (rotated meanwhile).
  const lastLayout = useRef<{ width: number; height: number } | null>(
    resume === undefined ? null : { width: 0, height: 0 },
  );
  const resizeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onLayout = useCallback(
    (event: LayoutChangeEvent) => {
      const { width, height } = event.nativeEvent.layout;
      const last = lastLayout.current;
      lastLayout.current = { width, height };
      // The first layout matches the size open() was given.
      if (last === null || (last.width === width && last.height === height)) {
        return;
      }
      if (resizeTimer.current !== null) {
        clearTimeout(resizeTimer.current);
      }
      resizeTimer.current = setTimeout(() => {
        resizeTimer.current = null;
        resize(width, height, PixelRatio.get());
      }, RESIZE_DEBOUNCE_MS);
    },
    [resize],
  );
  useEffect(
    () => () => {
      if (resizeTimer.current !== null) {
        clearTimeout(resizeTimer.current);
      }
    },
    [],
  );

  const sendClipboard = async () => {
    let text: string | null;
    try {
      text = await platform.readText();
    } catch {
      text = null;
    }
    if (text === null || text === '') {
      notify(adapters.toast.clipboardEmpty);
      return;
    }
    switch (session.sendClipboard(text)) {
      case 'sent':
        notify(adapters.toast.clipboardSent);
        break;
      case 'latin1Only':
        notify(adapters.toast.clipboardLatin1Only);
        break;
      case 'unsupported':
        notify(adapters.toast.clipboardUnsupported);
        break;
      case 'empty':
        notify(adapters.toast.clipboardEmpty);
        break;
      case 'failed':
        core.logs.write('warn', 'adapters', 'clipboard send failed');
        notify(adapters.toast.clipboardFailed);
        break;
    }
  };

  // A paste on the computer first gets this device's clipboard (setting
  // keyboard.pasteSync). Screen Sharing only carries Latin-1.
  const pushClipboardBeforePaste = async (): Promise<void> => {
    let text: string | null;
    try {
      text = await platform.readText();
    } catch {
      text = null;
    }
    if (text === null || text === '') {
      return;
    }
    switch (session.sendClipboard(text)) {
      case 'latin1Only':
        notify(adapters.toast.pasteLatin1Only);
        break;
      case 'failed':
        core.logs.write('warn', 'adapters', 'clipboard sync before paste failed');
        break;
      default:
        break;
    }
  };

  const typePassword = () => {
    notify(
      session.typePassword()
        ? adapters.toast.passwordTyped
        : adapters.toast.passwordTypeFailed,
    );
  };

  const sendCtrlAltDel = () => {
    setMoreOpen(false);
    notify(
      session.sendCtrlAltDel()
        ? adapters.toast.ctrlAltDelSent
        : adapters.toast.ctrlAltDelFailed,
    );
  };

  const copyRemoteClipboard = () => {
    setMoreOpen(false);
    if (remoteClipboard === null) {
      return;
    }
    platform.copyText(remoteClipboard).then(
      () => notify(adapters.toast.remoteClipboardCopied),
      () => notify(adapters.toast.clipboardFailed),
    );
  };

  const sendCombo = (combo: SessionCombo) => {
    if (sessionId === null) {
      return;
    }
    try {
      core.sessions.combo(sessionId, combo);
    } catch {
      core.logs.write('warn', 'adapters', 'combo failed');
      notify(adapters.toast.comboFailed);
      return;
    }
    if (haptics) {
      platform.haptic('light');
    }
  };

  const selectScreen = (index: number) => {
    const api = core.adapters;
    if (sessionId === null || api === undefined || index === screenIndex) {
      return;
    }
    try {
      api.selectScreen(sessionId, index);
    } catch {
      core.logs.write('warn', 'adapters', 'select screen failed');
      notify(adapters.toast.screenSwitchFailed);
      return;
    }
    setScreenIndex(index);
  };

  const changeMonitors = (count: MonitorCount) => {
    const api = core.adapters;
    if (sessionId === null || api === undefined || count === monitors) {
      return;
    }
    try {
      api.setMonitors(sessionId, count);
    } catch {
      core.logs.write('warn', 'adapters', 'set monitors failed');
      notify(adapters.monitors.changeFailed);
      return;
    }
    setMonitors(count);
    setMonitorChange(count);
    // The shown monitor goes away: back to the primary without a notice.
    if (screenIndex !== WHOLE_DESKTOP && screenIndex >= count) {
      setScreenIndex(0);
    }
  };

  const forget = () => {
    setConfirm(null);
    const api = core.adapters;
    if (api === undefined) {
      return;
    }
    try {
      api.forget(target);
    } catch {
      core.logs.write('warn', 'adapters', 'forget failed');
      notify(adapters.toast.forgetFailed);
      return;
    }
    setSaved(current => ({ ...current, info: NOT_SAVED }));
    notify(adapters.toast.forgotten);
  };

  const submit = (values: CredInput, save: boolean): boolean => {
    // V14: a failed attempt brings the sheet back with these filled in.
    setLastCreds({ user: values.user, domain: values.domain });
    return session.submit(values, save);
  };

  const trustCert = (accept: boolean) => {
    setCertFlow('answered');
    session.trustCert(accept);
  };

  const retry = (useSaved = true) => {
    setCertFlow('none');
    if (sessionId === null) {
      onReopen();
    } else {
      session.retry({ useSaved });
    }
  };

  const computer =
    target.hostId === undefined
      ? undefined
      : directory?.computers.find(c => c.hostId === target.hostId);
  // V37: only a known console state says the login window is gone.
  const hostReady =
    framed &&
    !hostReadyDismissed &&
    target.hostId !== undefined &&
    context !== undefined &&
    HOST_READY_CONTEXTS.includes(context) &&
    computer !== undefined &&
    computer.online &&
    computer.hostRunning &&
    computer.console != null &&
    computer.console.loginWindow === false;
  const switchToHost = () => {
    if (target.hostId === undefined) {
      return;
    }
    close();
    navigation.replace('Session', { hostId: target.hostId });
  };

  // this page's own session is not in the folded-away store.
  const currentItem: SwitcherItem | null =
    sessionId === null
      ? null
      : {
          kind: target.protocol,
          id: sessionId,
          params: routeParamsOf(params),
          name: adapterHistory.name,
          status: adapterSwitcherStatus(phase, reconnect.active),
          startedAtUs: adapterHistory.live?.startedAtUs ?? openedAtUs,
        };
  const switcherList = useSwitcherList(currentItem);
  // other windows see which session this page shows.
  useWindowPage(currentItem);
  const elsewhereIds = useElsewhereIds();
  const spaceList = useMemo(
    () => withoutElsewhere(switcherList, elsewhereIds),
    [elsewhereIds, switcherList],
  );
  const openSwitcher = () => {
    setMoreOpen(false);
    setSwitcherOpen(true);
  };
  // Folds this session away (handOff on unmount) and shows the other one.
  const switchTo = (item: SwitcherItem) => {
    setSwitcherOpen(false);
    if (item.id === sessionId || focusOtherWindow(item.id, windowId)) {
      return;
    }
    if (!onSwitchAway()) {
      return;
    }
    replaceWithSession(navigation, item);
  };
  const openNewWindow = (item: SwitcherItem) => {
    setSwitcherOpen(false);
    if (openInNewWindow(item, windowId) === 'moving') {
      // Folded away as the page goes; the window opens after.
      onCollapse();
    }
  };
  const connectOther = () => {
    setSwitcherOpen(false);
    onCollapse();
  };
  const spaces = useSpaces({
    list: spaceList,
    currentId: sessionId,
    routeSessionId: params.sessionId,
    framed: pictureShown,
    videoRef,
    switchTo,
  });
  const onShortcut = (event: SessionShortcutEvent) => {
    if (event.action === 'switcher') {
      openSwitcher();
      return;
    }
    // ⌃⌥⌘N counts as the switcher numbers; ←/→ skip other windows' ones.
    const next = shortcutTarget(
      event.action === 'index' ? switcherList.screens : spaceList.screens,
      sessionId,
      event,
    );
    if (next !== null) {
      // The same slide as three fingers, toward where the target sits.
      spaces.slideTo(next);
    }
  };

  const os = target.protocol === 'vnc' ? 'macos' : 'windows';
  const encrypted = isEncryptedPath(target.protocol, addrKind);
  const tailnetEnabled = useTailnetEnabled();
  const waited = useWaitedSeconds(waiting);
  const viewId = streaming
    ? sessionId
    : reconnect.active
    ? lastStreamingId
    : null;
  const coachCtx = {
    mode,
    os,
    setMode,
    closeCoach,
  } satisfies Pick<SessionCtx, 'mode' | 'os' | 'setMode' | 'closeCoach'>;
  // The Host page's pointer-lock chip reads only these.
  const pointerCtx = {
    sessionId: viewId,
    isPad,
    keyboardOpen,
    view: { overlay: 'none' },
  } satisfies Pick<SessionCtx, 'sessionId' | 'isPad' | 'keyboardOpen'> & {
    view: Pick<SessionCtx['view'], 'overlay'>;
  };

  return (
    <View
      onAccessibilityEscape={back}
      onLayout={onLayout}
      style={styles.body}
      testID="adapter-session-body"
    >
      {viewId !== null ? (
        <View style={StyleSheet.absoluteFill}>
          <RemoteSessionView
            accessibilityLabel={adapters.a11y.view(displayName)}
            cursorSpeed={cursorSpeed}
            haptics={haptics}
            keyboardInset={keyboardOpen ? inset : 0}
            mode={mode}
            naturalScroll={naturalScroll}
            onFirstFrame={onFirstFrame}
            onGesture={({ kind }) => {
              if (kind === 'threeFingerTap') {
                setCollapsed(value => !value);
              } else if (kind === 'twoFingerDoubleTap') {
                setMode(toggleMode);
              }
            }}
            onSpaceSwipe={spaces.onSpaceSwipe}
            onSpaceSwitch={spaces.onSpaceSwitch}
            onViewport={({ zoom }) => setZoomed(zoom > ZOOMED_IN)}
            reduceMotion={reduceMotion}
            ref={videoRef}
            sessionId={viewId}
            showSnapshot={spaces.showSnapshot}
            spaces={spaces.spaces}
            toolbarCollapsed={collapsed}
          />
        </View>
      ) : null}
      {spaces.indicator === null ? null : (
        <SpaceIndicator
          fading={spaces.indicator.fading}
          highlightId={spaces.indicator.highlightId}
          reduceMotion={reduceMotion}
          spaces={spaces.spaces}
        />
      )}
      {framed ? (
        <>
          <AdapterToolbar
            canTypePassword={target.protocol === 'vnc' && saved.info.saved}
            collapsed={collapsed}
            dock={dock}
            keyboardInset={keyboardOpen ? inset : 0}
            keyboardOpen={keyboardOpen}
            mode={mode}
            onCollapsedChange={setCollapsed}
            onDisconnect={() => setConfirm('disconnect')}
            onCombo={sendCombo}
            onDock={setDock}
            onHint={notify}
            onKeyboard={() => (keyboardOpen ? closeKeyboard() : openKeyboard())}
            onMinimize={onCollapse}
            onMore={() => setMoreOpen(true)}
            onSelectScreen={selectScreen}
            onSendClipboard={() => {
              sendClipboard().catch(() => undefined);
            }}
            onSwitch={openSwitcher}
            onToggleMode={() => setMode(toggleMode)}
            onTypePassword={typePassword}
            protocol={target.protocol}
            selectedScreen={screenIndex}
            shortcuts={padLayout}
            {...(screens.length > 1 ? { screens } : {})}
            {...(target.protocol === 'rdp'
              ? { monitors: { count: monitors, onChange: changeMonitors } }
              : {})}
          />
          <View pointerEvents="box-none" style={StyleSheet.absoluteFill}>
            <SessionKeyboard
              onBeforePaste={pushClipboardBeforePaste}
              onClose={closeKeyboard}
              onShortcut={onShortcut}
              open={keyboardOpen}
              os={os}
              sessionId={sessionId}
            />
          </View>
          <View pointerEvents="box-none" style={StyleSheet.absoluteFill}>
            <PointerLockChip ctx={pointerCtx as SessionCtx} />
          </View>
          {encrypted ? null : (
            <View
              accessibilityLabel={
                tailnetEnabled
                  ? adapters.unencryptedBadgeA11y
                  : adapters.unencryptedBadgeA11yLan
              }
              accessible
              style={styles.badge}
              testID="adapter-unencrypted-badge"
            >
              <Icon color="relay" name="warning" size={12} />
              <AppText color="text2" variant="caption">
                {adapters.unencryptedBadge}
              </AppText>
            </View>
          )}
        </>
      ) : null}
      {waiting ? (
        <View style={styles.waiting} testID="adapter-waiting">
          <View style={styles.waitingContent}>
            <ActivityIndicator color={tokens.text2} size="large" />
            <AppText
              accessibilityLiveRegion="polite"
              color="text"
              style={styles.waitingText}
              variant="headline"
            >
              {awaitingFrame
                ? adapters.loadingPicture
                : monitorChange !== null
                ? adapters.monitors.reconnecting(monitorChange)
                : phase === 'authenticating'
                ? adapters.authenticating
                : adapters.connecting(displayName)}
            </AppText>
            {waited >= SHOW_WAITED_AFTER_S ? (
              <AppText
                color="text2"
                style={styles.waitingText}
                testID="adapter-waited"
                variant="footnote"
              >
                {adapters.waited(waited)}
              </AppText>
            ) : null}
            {encrypted ? null : (
              <Banner
                icon="warning"
                testID="adapter-unencrypted"
                title={
                  tailnetEnabled ? adapters.unencrypted : adapters.unencryptedLan
                }
                tone="warn"
              />
            )}
            {target.protocol === 'rdp' ? (
              <Banner
                icon="info"
                testID="adapter-rdp-takeover"
                title={adapters.rdpTakeover}
                tone="info"
              />
            ) : null}
          </View>
          <View style={styles.waitingClose}>
            <Button
              onPress={disconnect}
              testID="adapter-wait-disconnect"
              title={adapters.cancel}
              variant="pill"
            />
          </View>
        </View>
      ) : null}
      {reconnect.active && phase !== 'needsCredentials' ? (
        <View style={styles.reconnect} testID="adapter-reconnecting">
          <View style={styles.reconnectCard}>
            <ActivityIndicator color={tokens.text2} />
            <AppText
              accessibilityLiveRegion="polite"
              color="text"
              variant="headline"
            >
              {adapters.reconnect.title}
            </AppText>
            <AppText color="text2" variant="footnote">
              {adapters.reconnect.attempt(reconnect.attempt, reconnect.max)}
            </AppText>
            <Button
              onPress={reconnect.cancel}
              testID="adapter-reconnect-cancel"
              title={adapters.reconnect.cancel}
              variant="secondary"
            />
          </View>
        </View>
      ) : null}
      {error !== null ? (
        <AdapterError
          certTimedOut={certFlow === 'asked'}
          error={error}
          name={displayName}
          onBack={onLeave}
          onCopyCode={() => onCopyCode(error.rsCode)}
          onOtherApp={onOtherApp}
          onRetry={() => retry()}
          protocol={target.protocol}
          {...(target.protocol === 'rdp' && error.code === 'nlaRequired'
            ? { onRetryWithPassword: () => retry(false) }
            : {})}
        />
      ) : null}
      {hostReady || (framed && takeoverNote) ? (
        <View pointerEvents="box-none" style={styles.topBanner}>
          <View style={styles.topBannerContent}>
            {hostReady ? (
              <View style={styles.solid}>
                <Banner
                  action={{
                    label: adapters.hostReady.action,
                    onPress: switchToHost,
                  }}
                  icon="check"
                  onDismiss={() => setHostReadyDismissed(true)}
                  testID="adapter-host-ready"
                  title={adapters.hostReady.message(displayName)}
                  tone="good"
                />
              </View>
            ) : null}
            {framed && takeoverNote ? (
              <View style={styles.solid}>
                <Banner
                  icon="info"
                  onDismiss={() => setTakeoverNote(false)}
                  testID="adapter-takeover-note"
                  title={adapters.rdpTakeover}
                  tone="info"
                />
              </View>
            ) : null}
          </View>
        </View>
      ) : null}
      {phase === 'needsCredentials' ? (
        <CredentialSheet
          canSave={canSave}
          defaultDomain={lastCreds.domain ?? saved.info.domain}
          defaultUser={lastCreds.user ?? saved.info.user}
          failed={state.credError}
          fields={state.credFields}
          onCancel={session.cancel}
          onSubmit={submit}
          protocol={target.protocol}
        />
      ) : null}
      {state.cert !== null ? (
        <CertPrompt
          fp={state.cert.fp}
          key={state.cert.fp}
          onAnswer={trustCert}
          {...(state.cert.oldFp === undefined
            ? {}
            : { oldFp: state.cert.oldFp })}
        />
      ) : null}
      {coachVisible && framed ? (
        <View
          pointerEvents="box-none"
          style={StyleSheet.absoluteFill}
          testID="adapter-coach"
        >
          <GestureCoach ctx={coachCtx as SessionCtx} />
        </View>
      ) : null}
      {switcherOpen && framed ? (
        <SessionSwitcher
          currentId={sessionId}
          currentNewWindow={windowId === MAIN_WINDOW}
          elsewhereIds={elsewhereIds}
          list={switcherList}
          onClose={() => setSwitcherOpen(false)}
          onOther={connectOther}
          onSelect={switchTo}
          shortcuts={isPad}
          {...(windowsSupported() ? { onNewWindow: openNewWindow } : {})}
        />
      ) : null}
      {moreOpen && framed ? (
        <AdapterMoreSheet
          canForget={saved.info.saved}
          hasRemoteClipboard={!autoSync && remoteClipboard !== null}
          onClose={() => setMoreOpen(false)}
          onCopyRemoteClipboard={copyRemoteClipboard}
          onCtrlAltDel={sendCtrlAltDel}
          onForget={() => {
            setMoreOpen(false);
            setConfirm('forget');
          }}
          onGestures={() => {
            setMoreOpen(false);
            setCoachVisible(true);
          }}
          {...(zoomed
            ? {
                onFitScreen: () => {
                  setMoreOpen(false);
                  videoRef.current?.zoomReset();
                },
              }
            : {})}
          onSound={setSound}
          protocol={target.protocol}
          sound={sound}
        />
      ) : null}
      <Dialog
        actions={[
          {
            label: adapters.disconnect.confirm,
            onPress: () => {
              setConfirm(null);
              disconnect();
            },
            testID: 'adapter-disconnect-confirm',
            variant: 'pillDanger',
          },
          {
            label: adapters.disconnect.cancel,
            onPress: () => setConfirm(null),
            testID: 'adapter-disconnect-cancel',
            variant: 'text',
          },
        ]}
        message={adapters.disconnect.message}
        onDismiss={() => setConfirm(null)}
        testID="adapter-disconnect-dialog"
        title={adapters.disconnect.title(displayName)}
        visible={confirm === 'disconnect'}
      />
      <Dialog
        actions={[
          {
            label: adapters.forget.confirm,
            onPress: forget,
            testID: 'adapter-forget-confirm',
            variant: 'pillDanger',
          },
          {
            label: adapters.forget.cancel,
            onPress: () => setConfirm(null),
            testID: 'adapter-forget-cancel',
            variant: 'text',
          },
        ]}
        message={adapters.forget.message}
        onDismiss={() => setConfirm(null)}
        testID="adapter-forget-dialog"
        title={adapters.forget.title}
        visible={confirm === 'forget'}
      />
    </View>
  );
}

/**
 * A switch to another screen-sharing / remote-desktop session swaps the
 * route's params in place; the key remounts the page for it,
 * as a new page would.
 */
export default function AdapterSessionScreen(
  props: RootScreenProps<'AdapterSession'>,
) {
  return (
    <AdapterSessionPage key={props.route.params.switchSeq ?? 0} {...props} />
  );
}

function AdapterSessionPage({
  navigation,
  route,
}: RootScreenProps<'AdapterSession'>) {
  const params = route.params;
  const { target } = params;
  const displayName = params.name ?? target.addr;
  const core = useCore();
  const platform = usePlatform();
  const windowId = useWindowId();
  // on screen in another window (asked for by id or by computer):
  // that window comes forward and this page goes without touching it.
  const [elsewhere] = useState(() =>
    windowsSupported()
      ? otherWindowShowing(windows.getState(), windowId, params.sessionId, target)
      : null,
  );
  const { toast, show, hide } = useToast();
  const { open: openFallback, guide, closeGuide } = useRemoteFallback();
  const [available, setAvailable] = useState(() => adaptersAvailable(core));
  // Bumped to remount the session when there is no adapter id to retry.
  const [attempt, setAttempt] = useState(0);
  const left = useRef(false);
  const [beforeSession] = useSetting('lock.beforeSession');
  const [sensitiveHosts] = useSetting('lock.sensitiveHosts');
  // S03: the same App lock gate as Host sessions, checked once on entry.
  const [unlocked, setUnlocked] = useState(
    () =>
      !needsSessionAuth(target.hostId ?? '', {
        beforeSession,
        sensitiveHosts,
      }),
  );

  const leave = useCallback(() => {
    if (left.current) {
      return;
    }
    left.current = true;
    navigation.goBack();
  }, [navigation]);
  // V33: Home's bar and the session tab bring it back.
  const collapse = useCallback(() => {
    if (left.current) {
      return;
    }
    left.current = true;
    // Back down to the tabs, not a second copy on top: the page goes, so the
    // session folds away and its picture pauses.
    navigation.popTo('MainTabs', { screen: 'Home' });
  }, [navigation]);
  // replaced by another session from the switcher.
  const switchAway = useCallback(() => {
    if (left.current) {
      return false;
    }
    left.current = true;
    return true;
  }, []);
  // Picks up the folded-away session for this computer, asked for by id
  // (Home's bar, the session tab) or by tapping the computer again.
  const [resumeId] = useState(() => {
    const live = adapterLive.getState();
    const byId =
      params.sessionId === undefined ? undefined : live[params.sessionId];
    return (byId ?? findCollapsed(live, target))?.sessionId ?? null;
  });

  useEffect(() => {
    if (unlocked || !available) {
      return;
    }
    let current = true;
    platform
      .authenticate(adapters.authenticate(displayName))
      .catch(() => 'failed' as const)
      .then(result => {
        if (!current) {
          return;
        }
        if (result === 'cancelled' || result === 'failed') {
          leave();
        } else {
          setUnlocked(true);
        }
      });
    return () => {
      current = false;
    };
  }, [available, displayName, leave, platform, unlocked]);

  const openOtherApp = useCallback(() => {
    openFallback({ ...fallbackTargetFor(target), preferExternal: true }).catch(
      () => undefined,
    );
  }, [openFallback, target]);

  const copyCode = useCallback(
    (rsCode: number) => {
      platform.copyText(errorCodeLabel(rsCode)).catch(() => undefined);
      show(adapters.toast.codeCopied);
    },
    [platform, show],
  );

  useEffect(() => {
    if (elsewhere !== null) {
      activateWindow(elsewhere);
      leave();
    }
  }, [elsewhere, leave]);

  let content: JSX.Element;
  if (elsewhere !== null) {
    content = <View style={styles.body} testID="adapter-elsewhere" />;
  } else if (!available) {
    // V45: this build has no adapters; retrying cannot help.
    content = (
      <AdapterError
        error={UNAVAILABLE}
        name={displayName}
        onBack={leave}
        onCopyCode={() => copyCode(UNAVAILABLE.rsCode)}
        onOtherApp={openOtherApp}
        onRetry={() => setAvailable(adaptersAvailable(core))}
        protocol={target.protocol}
        unavailable
      />
    );
  } else if (!unlocked) {
    content = <View style={styles.body} testID="adapter-locked" />;
  } else {
    content = (
      <AdapterSessionBody
        displayName={displayName}
        key={attempt}
        navigation={navigation}
        notify={show}
        onCollapse={collapse}
        onCopyCode={copyCode}
        onSwitchAway={switchAway}
        onLeave={leave}
        onOtherApp={openOtherApp}
        onReopen={() => setAttempt(n => n + 1)}
        params={params}
        // A retry after the first attempt connects again.
        resumeId={attempt === 0 ? resumeId : null}
      />
    );
  }

  return (
    <View style={styles.root} testID="adapter-session">
      {content}
      <FallbackGuide guide={guide} onClose={closeGuide} />
      <Toast onHide={hide} position="top" toast={toast} />
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: '#000' },
  body: { flex: 1 },
  waiting: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#000',
  },
  waitingContent: {
    width: '100%',
    maxWidth: 520,
    gap: 16,
    paddingHorizontal: hit.landscapeSafe,
  },
  waitingText: { textAlign: 'center' },
  waitingClose: {
    position: 'absolute',
    top: 16,
    right: hit.landscapeSafe,
  },
  reconnect: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(0, 0, 0, 0.55)',
  },
  reconnectCard: {
    alignItems: 'center',
    gap: 12,
    minWidth: 240,
    maxWidth: 360,
    padding: 20,
    borderRadius: radius.card,
    backgroundColor: tokens.panel,
  },
  badge: {
    position: 'absolute',
    top: 12,
    left: hit.landscapeSafe,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: radius.pill,
    backgroundColor: tokens.hud,
  },
  topBanner: {
    position: 'absolute',
    top: 12,
    left: hit.landscapeSafe,
    right: hit.landscapeSafe,
    alignItems: 'center',
  },
  topBannerContent: { width: '100%', maxWidth: 560, gap: 8 },
  // V22: banners over the picture need an opaque backing to stay legible.
  solid: { borderRadius: radius.card, backgroundColor: tokens.card },
});
