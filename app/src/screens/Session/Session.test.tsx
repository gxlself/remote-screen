import { StrictMode, isValidElement } from 'react';
import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
} from '@testing-library/react-native';
import { AccessibilityInfo, StyleSheet, View } from 'react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { RootStackParamList } from '../../app/navigation/routes';
import * as GlobalToast from '../../components/ToastHost';
import {
  adapterLive,
  resetAdapterLive,
} from '../../features/adapters/liveStore';
import {
  markArrival,
  peekArrival,
  resetArrival,
} from '../../features/session/spaces';
import { useSessions } from '../../features/session/store';
import { videoStore } from '../../features/session/videoStore';
import {
  resetKeyboardInset,
  setKeyboardInset,
} from '../../native/keyboard/keyboardInset';
import type { SessionKeyboardProps } from '../../native/keyboard/SessionKeyboard';
import { FakePlatform } from '../../native/platform/fakePlatform';
import type { AuthResult } from '../../native/platform/PlatformServices';
import * as system from '../../native/platform/impl/system';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { liveSession, directory } from '../../native/rscore/fixtures';
import type { CoreEventOf, SessionPhase } from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { mockAnimateSpace } from '../../test/mocks/RemoteSessionViewMock';
import SessionScreen from './index';
import * as ClipboardSync from './ClipboardSync';
import * as ConnectingOverlay from './ConnectingOverlay';
import * as GestureCoach from './GestureCoach';
import * as IdleOverlay from './IdleOverlay';
import * as LocalUseBar from './LocalUseBar';
import * as PathNotice from './PathNotice';
import * as SessionHud from './SessionHud';
import * as Toolbar from './SessionToolbar';
import * as SharedOverlay from './SharedOverlay';
import type { VideoSlotProps } from './slots';
import type { KeyboardSlotProps } from './slots';
import { belowTopStack } from './topStack';
import { useSessionController } from './useSessionController';
import type { SessionController } from './useSessionController';
import * as VideoSlot from './VideoSlot';
import * as KeyboardSlot from './KeyboardSlot';

const mockKeyboardProps = jest.fn<void, [SessionKeyboardProps]>();

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

jest.mock('../../native/platform/impl/system', () => ({
  ...jest.requireActual('../../native/platform/impl/system'),
  maxFps: jest.fn(),
}));

jest.mock('../../native/keyboard/SessionKeyboard', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function SessionKeyboard(props: SessionKeyboardProps) {
    mockKeyboardProps(props);
    return React.createElement(MockView, { testID: 'session-keyboard-mock' });
  }
  return { __esModule: true, default: SessionKeyboard, SessionKeyboard };
});

const hostId = 'h7v2m9q3k8x4tb1a';
const hostName = 'Studio Mac mini';
const clients: QueryClient[] = [];
type Options = NonNullable<Parameters<typeof renderWithProviders>[1]>;
const stats: CoreEventOf<'session.stats'> = {
  type: 'session.stats',
  sessionId: 's1',
  rttMs: 6,
  fps: 60,
  bitrateKbps: 8000,
  decodeMs: 3,
  lossPct: 0,
  width: 2560,
  height: 1440,
  codec: 'hevc',
  path: 'tailnetDirect',
  transport: 'quic',
};

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function confirmDisconnect() {
  await fireEvent.press(
    within(screen.getByTestId('disconnect-confirm-actions')).getByText('断开'),
  );
}

async function mount(
  params: Partial<RootStackParamList['Session']> = {},
  opts: Options = {},
  strict = false,
) {
  const core = opts.core ?? createFakeCore({ sessions: [] });
  const navigation = mockNavigation<'Session'>();
  const route = mockRoute('Session', { hostId, ...params });
  const ui = <SessionScreen navigation={navigation} route={route} />;
  const view = await renderWithProviders(
    strict ? <StrictMode>{ui}</StrictMode> : ui,
    // The tailnet gives the Host its node name for the terminal action, so
    // 外网访问 (Tailscale) is on.
    {
      localDirectory: { peers: [] },
      ...opts,
      settings: { 'tailnet.enabled': true, ...opts.settings },
      core,
    },
  );
  clients.push(view.queryClient);
  await advance();
  return { ...view, navigation };
}

async function state(core: FakeCore, phase: SessionPhase, id = 's1') {
  await act(() =>
    core.emit({ type: 'session.state', sessionId: id, hostId, phase }),
  );
}

async function stream(core: FakeCore) {
  await state(core, 'streaming');
  await act(() => core.emit(stats));
  await fireEvent(
    screen.getByTestId('remote-session-view-placeholder'),
    'onFirstFrame',
  );
}

async function mountController() {
  let ctx: SessionController;
  const navigation = mockNavigation<'Session'>();
  function Probe() {
    ctx = useSessionController({
      navigation,
      route: mockRoute('Session', { hostId }),
    });
    return null;
  }
  const view = await renderWithProviders(<Probe />, {
    core: createFakeCore({ sessions: [] }),
  });
  clients.push(view.queryClient);
  await advance();
  return { ...view, controller: () => ctx };
}

type TreeNode = { type: unknown; parent: unknown };

/** Host element types from `element` up to the root (Modal shows as one). */
function hostTypesAbove(element: TreeNode): string[] {
  const types: string[] = [];
  let node: TreeNode | null = element;
  while (node !== null && node !== undefined) {
    types.push(String(node.type));
    node = node.parent as TreeNode | null;
  }
  return types;
}

function keyboardProps(): SessionKeyboardProps {
  const calls = mockKeyboardProps.mock.calls;
  const props = calls[calls.length - 1]?.[0];
  if (props === undefined) {
    throw new Error('SessionKeyboard was not rendered');
  }
  return props;
}

beforeEach(() => {
  jest.useFakeTimers();
  resetKeyboardInset();
  mockKeyboardProps.mockClear();
  videoStore.setState({ stats: {}, viewport: {} });
});
afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('opens once and keeps the connecting layer after stats until the native first frame', async () => {
  // This test verifies the shell layers, not the visual content owned by each
  // slot. Stubbing the heavy children keeps the full-screen mount cheap under
  // parallel test load; their own suites cover the rendering.
  for (const slot of [
    ClipboardSync,
    ConnectingOverlay,
    GestureCoach,
    IdleOverlay,
    LocalUseBar,
    PathNotice,
    SessionHud,
    SharedOverlay,
    Toolbar,
  ]) {
    jest.spyOn(slot, 'default').mockImplementation(() => null);
  }
  const view = await mount();
  expect(
    view.core.calls.filter(call => call.method === 'sessions.open'),
  ).toEqual([
    {
      method: 'sessions.open',
      args: [
        hostId,
        { intent: 'control', quality: 'auto', mode: 'trackpad', audio: false },
      ],
    },
  ]);
  expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
  expect(view.queryByTestId('layer-toolbar')).toBeNull();
  await state(view.core, 'streaming');
  expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
  await act(() => view.core.emit({ ...stats, sessionId: 'another-session' }));
  expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
  await act(() => view.core.emit(stats));
  expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
  await fireEvent(
    view.getByTestId('remote-session-view-placeholder'),
    'onFirstFrame',
  );
  expect(view.queryByTestId('layer-connecting')).toBeNull();
  expect(view.getByTestId('layer-hud')).toBeOnTheScreen();
  expect(view.getByTestId('layer-toolbar')).toBeOnTheScreen();
  expect(view.getByTestId('layer-status')).toBeOnTheScreen();
  expect(
    view.getByLabelText('Studio Mac mini 远端画面，双指双击切换直接触控'),
  ).toHaveProp('accessible', true);
  expect(view.getByTestId('remote-session-view-placeholder')).toHaveProp(
    'accessibilityRole',
    'image',
  );
  await view.unmount();
  expect(view.core.calls.some(call => call.method === 'sessions.close')).toBe(
    false,
  );
});

it('passes the native maximum frame rate when opening a session', async () => {
  jest.mocked(system.maxFps).mockReturnValueOnce(120);
  const view = await mount();
  expect(view.core.calls).toContainEqual({
    method: 'sessions.open',
    args: [
      hostId,
      {
        intent: 'control',
        quality: 'auto',
        mode: 'trackpad',
        audio: false,
        maxFps: 120,
      },
    ],
  });
});

it('uses view intent, tablet defaults and an explicit display', async () => {
  const platform = new FakePlatform();
  platform.info.isPad = true;
  const view = await mount({ viewOnly: true, displayId: 2 }, { platform });
  expect(view.core.calls).toContainEqual({
    method: 'sessions.open',
    args: [
      hostId,
      {
        intent: 'view',
        quality: 'auto',
        mode: 'touch',
        audio: false,
        displayId: 2,
      },
    ],
  });
});

it('uses the configured mode and quality without persisting session-only mode changes', async () => {
  const view = await mount(
    {},
    {
      settings: {
        'quality.default': 'clear',
        'session.mode.phone': 'touch',
        'session.sound': true,
      },
    },
  );
  expect(view.core.calls).toContainEqual({
    method: 'sessions.open',
    args: [
      hostId,
      { intent: 'control', quality: 'clear', mode: 'touch', audio: true },
    ],
  });
  await fireEvent(
    view.getByTestId('remote-session-view-placeholder'),
    'gesture',
    { kind: 'twoFingerDoubleTap' },
  );
  expect(view.settingsStore.get('session.mode.phone')).toBe('touch');
  expect(
    view.getByLabelText('Studio Mac mini 远端画面，双指双击切换直接触控'),
  ).toBeOnTheScreen();
});

it('attaches an existing live record without opening or authenticating again', async () => {
  const core = createFakeCore({ sessions: [liveSession] });
  const view = await mount(
    { sessionId: liveSession.id },
    { core, settings: { 'lock.beforeSession': true } },
  );
  expect(core.calls.some(call => call.method === 'sessions.open')).toBe(false);
  expect(view.platform.calls.some(call => call.method === 'authenticate')).toBe(
    false,
  );
  expect(view.getByTestId('layer-hud')).toBeOnTheScreen();
  expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
  await fireEvent(
    view.getByTestId('remote-session-view-placeholder'),
    'onFirstFrame',
  );
  expect(view.queryByTestId('layer-connecting')).toBeNull();
});

it('opens a new session if the requested record has ended or is missing', async () => {
  const core = createFakeCore({
    sessions: [{ ...liveSession, phase: 'ended' }],
  });
  await mount({ sessionId: liveSession.id }, { core });
  expect(
    core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
});

it.each<AuthResult>(['cancelled', 'failed'])(
  'returns without opening when sensitive-host authentication is %s',
  async authResult => {
    const platform = new FakePlatform();
    platform.authResult = authResult;
    const view = await mount(
      {},
      { platform, settings: { 'lock.sensitiveHosts': [hostId] } },
    );
    expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
    expect(view.core.calls.some(call => call.method === 'sessions.open')).toBe(
      false,
    );
  },
);

it.each<AuthResult>(['ok', 'unavailable'])(
  'authenticates before opening and proceeds for %s',
  async authResult => {
    const platform = new FakePlatform();
    platform.authResult = authResult;
    const core = createFakeCore({ sessions: [] });
    const authenticate = jest.spyOn(platform, 'authenticate');
    const open = jest.spyOn(core.sessions, 'open');
    await mount(
      {},
      { core, platform, settings: { 'lock.sensitiveHosts': [hostId] } },
    );
    expect(authenticate).toHaveBeenCalledWith('验证身份后连接 Studio Mac mini');
    expect(authenticate.mock.invocationCallOrder[0]).toBeLessThan(
      open.mock.invocationCallOrder[0],
    );
  },
);

it('also authenticates when the global before-session setting is enabled', async () => {
  const view = await mount({}, { settings: { 'lock.beforeSession': true } });
  expect(view.platform.calls).toContainEqual({
    method: 'authenticate',
    args: ['验证身份后连接 Studio Mac mini'],
  });
});

it('does not open twice during StrictMode effect replay', async () => {
  const view = await mount(
    {},
    { settings: { 'lock.beforeSession': true } },
    true,
  );
  expect(
    view.core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
  expect(
    view.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(1);
});

it('routes errors once with their details and dismisses the ended record', async () => {
  let exists: boolean | undefined;
  function Observe() {
    exists = useSessions(sessions => sessions.s1 !== undefined);
    return null;
  }
  const core = createFakeCore({ sessions: [] });
  const navigation = mockNavigation<'Session'>();
  const view = await renderWithProviders(
    <>
      <SessionScreen
        navigation={navigation}
        route={mockRoute('Session', { hostId })}
      />
      <Observe />
    </>,
    { core },
  );
  clients.push(view.queryClient);
  await advance();
  await state(core, 'pendingApproval');
  expect(exists).toBe(true);
  const error: CoreEventOf<'session.error'> = {
    type: 'session.error',
    sessionId: 's1',
    hostId,
    code: 140,
    detail: { cause: 'denied' },
  };
  await act(() => core.emit(error));
  await act(() => core.emit(error));
  expect(navigation.replace).toHaveBeenCalledTimes(1);
  expect(navigation.replace).toHaveBeenCalledWith('SessionEnded', {
    hostId,
    hostName,
    sessionId: 's1',
    code: 140,
    detail: { cause: 'denied' },
  });
  expect(exists).toBe(false);
});

it('returns immediately for userClosed', async () => {
  const view = await mount();
  await state(view.core, 'connecting');
  await act(() =>
    view.core.emit({
      type: 'session.error',
      sessionId: 's1',
      hostId,
      reason: 'userClosed',
    }),
  );
  expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
  expect(view.navigation.replace).not.toHaveBeenCalled();
});

it('returns right away and carries the shutdown message along, exactly once', async () => {
  // 用全局 Toast，提示跟着回到上一页，不在黑屏上等两秒。
  const toast = jest.spyOn(GlobalToast, 'showToast');
  const view = await mount();
  await stream(view.core);
  const error: CoreEventOf<'session.error'> = {
    type: 'session.error',
    sessionId: 's1',
    hostId,
    reason: 'hostShutdown',
  };
  await act(() => view.core.emit(error));
  await act(() => view.core.emit(error));
  expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
  expect(toast).toHaveBeenCalledTimes(1);
  expect(toast).toHaveBeenCalledWith({ message: '电脑正在关机或重启' });
  GlobalToast.hideToast();
});

it('does not leave a second time after the screen unmounts', async () => {
  const view = await mount();
  await stream(view.core);
  await act(() =>
    view.core.emit({
      type: 'session.error',
      sessionId: 's1',
      hostId,
      reason: 'hostShutdown',
    }),
  );
  await view.unmount();
  await advance(2000);
  expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
  GlobalToast.hideToast();
});

it('routes an open rejection to SessionEnded', async () => {
  const core = createFakeCore({ sessions: [] });
  core.failNext('sessions.open', new Error('boom'));
  const view = await mount({}, { core });
  expect(view.navigation.replace).toHaveBeenCalledWith('SessionEnded', {
    hostId,
    hostName,
    detail: { message: 'boom' },
  });
});

it('background expiry returns with a toast and does not reopen', async () => {
  const toast = jest.spyOn(GlobalToast, 'showToast');
  const view = await mount();
  await stream(view.core);
  await act(() =>
    view.core.emit({
      type: 'session.error',
      sessionId: 's1',
      hostId,
      reason: 'background',
    }),
  );
  expect(toast).toHaveBeenCalledWith({ message: '在后台太久，会话已断开' });
  GlobalToast.hideToast();
  expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
  expect(view.navigation.replace).not.toHaveBeenCalled();
  expect(
    view.core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
});

it('shows a route notice once in a top toast', async () => {
  const notice = '没打开屏幕共享，已改用 Host 连接';
  const view = await mount({ notice });

  expect(view.getByTestId('toast')).toHaveProp('accessibilityLabel', notice);
  expect(view.getByText(notice)).toBeOnTheScreen();
  const layer = view.getByTestId('toast').parent;
  // R37：顶部 Toast 排在 HUD 那一行和顶部提示栏下面。
  expect(StyleSheet.flatten(layer?.props.style).top).toBe(belowTopStack(0));

  await advance(3000);
  expect(view.queryByTestId('toast')).toBeNull();

  // Re-rendering the same route and reconnecting do not raise it again.
  await view.rerender(
    <SessionScreen
      navigation={view.navigation}
      route={mockRoute('Session', { hostId, notice })}
    />,
  );
  await stream(view.core);
  await state(view.core, 'reconnecting');
  await state(view.core, 'streaming');
  await advance();
  expect(view.queryByText(notice)).toBeNull();
});

it('shows no toast without a route notice', async () => {
  const view = await mount();

  expect(view.queryByTestId('toast')).toBeNull();
  await stream(view.core);
  expect(view.queryByTestId('toast')).toBeNull();
});

it('toggles toolbar visibility and input mode through the video gestures', async () => {
  const view = await mount();
  await stream(view.core);
  const video = view.getByTestId('remote-session-view-placeholder');
  await fireEvent(video, 'gesture', { kind: 'threeFingerTap' });
  expect(view.queryByTestId('layer-toolbar')).toBeNull();
  await fireEvent(video, 'gesture', { kind: 'threeFingerTap' });
  expect(view.getByTestId('layer-toolbar')).toBeOnTheScreen();
  await fireEvent(video, 'gesture', { kind: 'twoFingerDoubleTap' });
  expect(
    view.getByLabelText('Studio Mac mini 远端画面，双指双击切换触控板'),
  ).toBeOnTheScreen();
  await fireEvent(video, 'gesture', { kind: 'twoFingerDoubleTap' });
  expect(
    view.getByLabelText('Studio Mac mini 远端画面，双指双击切换直接触控'),
  ).toBeOnTheScreen();
});

it('accepts a native first-frame callback before stats', async () => {
  const view = await mount();
  await state(view.core, 'streaming');
  await fireEvent(
    view.getByTestId('remote-session-view-placeholder'),
    'firstFrame',
  );
  expect(view.queryByTestId('layer-connecting')).toBeNull();
});

it('warns once per session after eight seconds of streaming without a native first frame', async () => {
  const view = await mountController();
  await advance(8000);
  expect(view.core.calls.filter(call => call.method === 'logs.write')).toEqual(
    [],
  );
  await state(view.core, 'streaming');
  await advance(7999);
  await act(() => view.core.emit(stats));
  expect(view.core.calls.filter(call => call.method === 'logs.write')).toEqual(
    [],
  );
  await advance(1);
  expect(view.core.calls.filter(call => call.method === 'logs.write')).toEqual([
    { method: 'logs.write', args: ['warn', 'session', 'no first frame 8s'] },
  ]);
  await state(view.core, 'reconnecting');
  await state(view.core, 'streaming');
  await advance(8000);
  expect(
    view.core.calls.filter(call => call.method === 'logs.write'),
  ).toHaveLength(1);
});

it.each(['firstFrame', 'reconnecting', 'ended', 'unmount'] as const)(
  'cancels the missing-first-frame warning on %s',
  async action => {
    const view = await mountController();
    await state(view.core, 'streaming');
    await advance(7999);
    if (action === 'firstFrame') {
      await act(() => view.controller().onFirstFrame());
    } else if (action === 'unmount') {
      await view.unmount();
    } else {
      await state(view.core, action);
    }
    await advance(8001);
    expect(
      view.core.calls.filter(call => call.method === 'logs.write'),
    ).toEqual([]);
  },
);

it('exposes all native view commands through the controller video ref', async () => {
  let ctx: Parameters<typeof Toolbar.default>[0]['ctx'] | undefined;
  jest.spyOn(Toolbar, 'default').mockImplementation(props => {
    ctx = props.ctx;
    return null;
  });
  const view = await mount();
  await stream(view.core);
  const handle = ctx?.videoRef.current;
  expect(handle).not.toBeNull();
  handle?.zoomReset();
  handle?.centerCursor();
  handle?.showMagnifier(true);
  expect(handle?.zoomReset).toHaveBeenCalledTimes(1);
  expect(handle?.centerCursor).toHaveBeenCalledTimes(1);
  expect(handle?.showMagnifier).toHaveBeenCalledWith(true);
});

it.each([
  'collapse',
  'disconnect',
  'openTerminal',
  'ended',
  'background',
  'unmount',
] as const)(
  'clears only the current session video data on %s',
  async action => {
    let ctx: Parameters<typeof Toolbar.default>[0]['ctx'] | undefined;
    jest.spyOn(Toolbar, 'default').mockImplementation(props => {
      ctx = props.ctx;
      return null;
    });
    const view = await mount();
    await stream(view.core);
    const video = view.getByTestId('remote-session-view-placeholder');
    const nativeStats = {
      fps: 60,
      decodeP50Ms: 1,
      decodeP95Ms: 2,
      latencyP50Ms: -1,
      queue: 0,
      dropped: 0,
      keyframeRequests: 0,
      width: 1920,
      height: 1080,
      codec: 'hevc',
    };
    const viewport = { zoom: 2, x: 0.25, y: 0.5 };
    await fireEvent(video, 'onVideoStats', nativeStats);
    await fireEvent(video, 'onViewport', viewport);
    expect(videoStore.getState()).toEqual({
      stats: { s1: nativeStats },
      viewport: { s1: viewport },
    });
    videoStore.setState(data => ({
      stats: { ...data.stats, s2: nativeStats },
      viewport: { ...data.viewport, s2: viewport },
    }));
    if (action === 'unmount') {
      await view.unmount();
    } else if (action === 'ended') {
      await state(view.core, 'ended');
    } else if (action === 'background') {
      await act(() =>
        view.core.emit({
          type: 'session.error',
          sessionId: 's1',
          hostId,
          reason: 'background',
        }),
      );
    } else {
      await act(() => ctx?.[action]());
      if (action === 'disconnect') {
        // R02：已连接时断开要先确认。
        await confirmDisconnect();
      }
    }
    expect(videoStore.getState()).toEqual({
      stats: { s2: nativeStats },
      viewport: { s2: viewport },
    });
    if (action !== 'disconnect') {
      expect(
        view.core.calls.some(call => call.method === 'sessions.close'),
      ).toBe(false);
    }
  },
);

it.each([false, true])(
  'shows first-time coaching only when coachShown is false (%s)',
  async coachShown => {
    const view = await mount(
      {},
      { settings: { 'session.coachShown': coachShown } },
    );
    await stream(view.core);
    expect(view.queryByTestId('layer-coach') !== null).toBe(!coachShown);
  },
);

it('sets lockOnEnd only once when the session first streams', async () => {
  const view = await mount(
    {},
    { settings: { 'hosts.prefs': { [hostId]: { lockOnEnd: true } } } },
  );
  await stream(view.core);
  await state(view.core, 'reconnecting');
  await state(view.core, 'streaming');
  expect(
    view.core.calls.filter(call => call.method === 'sessions.action'),
  ).toEqual([{ method: 'sessions.action', args: ['s1', 'lockOnEnd', true] }]);
});

it('mounts the approval and reconnect layers at their fixed positions', async () => {
  const view = await mount();
  await state(view.core, 'pendingApproval');
  expect(view.getByTestId('layer-approval')).toBeOnTheScreen();
  expect(view.queryByTestId('layer-connecting')).toBeNull();
  expect(view.queryByTestId('layer-toolbar')).toBeNull();
  await state(view.core, 'reconnecting');
  expect(view.getByTestId('layer-reconnect')).toBeOnTheScreen();
  expect(view.queryByTestId('layer-approval')).toBeNull();
});

it('keeps the keyboard layer mounted and closes the keyboard slot through the toolbar', async () => {
  const view = await mount();
  expect(view.getByTestId('layer-keyboard')).toBeOnTheScreen();
  expect(keyboardProps().open).toBe(false);
  await stream(view.core);
  await fireEvent.press(view.getByLabelText('打开键盘'));
  expect(keyboardProps().open).toBe(true);
  await act(() => keyboardProps().onClose());
  expect(keyboardProps().open).toBe(false);
  expect(view.getByTestId('layer-keyboard')).toBeOnTheScreen();
});

it('passes the follow-zoom chip as the keyboard slot leading content', async () => {
  let received: KeyboardSlotProps | undefined;
  jest.spyOn(KeyboardSlot, 'default').mockImplementation(props => {
    received = props;
    return <View testID="keyboard-slot-probe" />;
  });
  const view = await mount();
  await stream(view.core);
  await fireEvent.press(view.getByLabelText('打开键盘'));
  expect(received?.open).toBe(true);
  expect(isValidElement(received?.leading)).toBe(true);
});

it('passes the keyboard inset from P5B through to the video slot', async () => {
  let videoProps: VideoSlotProps | undefined;
  jest.spyOn(VideoSlot, 'default').mockImplementation(props => {
    videoProps = props;
    return <View testID="video-inset-probe" />;
  });
  await mount();
  expect(videoProps?.keyboardInset).toBe(0);
  await act(() => setKeyboardInset(300));
  expect(videoProps?.keyboardInset).toBe(300);
});

it('wires sheet, coach, quality, collapse and terminal actions through the slot contract', async () => {
  let ctx: Parameters<typeof Toolbar.default>[0]['ctx'];
  jest.spyOn(Toolbar, 'default').mockImplementation(props => {
    ctx = props.ctx;
    return null;
  });
  const view = await mount();
  await stream(view.core);
  for (const kind of ['quality', 'displays', 'clipboard', 'more'] as const) {
    await act(() => ctx.openSheet(kind));
    expect(view.getByTestId('layer-sheet')).toBeOnTheScreen();
    await act(() => ctx.closeSheet());
    expect(view.queryByTestId('layer-sheet')).toBeNull();
  }
  await act(() => ctx.closeCoach());
  expect(view.queryByTestId('layer-coach')).toBeNull();
  expect(view.settingsStore.get('session.coachShown')).toBe(true);
  await act(() => ctx.openCoach());
  expect(view.getByTestId('layer-coach')).toBeOnTheScreen();
  await act(() => ctx.toggleKeyboard());
  expect(keyboardProps().open).toBe(true);
  await act(() => ctx.toggleKeyboard());
  expect(keyboardProps().open).toBe(false);
  await act(() => ctx.setQuality('eco'));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.setQuality',
    args: ['s1', 'eco'],
  });
  expect(view.settingsStore.get('quality.default')).toBe('auto');
  await act(() => ctx.collapse());
  expect(view.navigation.popTo).toHaveBeenCalledWith('MainTabs', {
    screen: 'Sessions',
  });
  await act(() => ctx.openTerminal());
  expect(view.navigation.navigate).toHaveBeenCalledWith('Terminal', {
    serverId: directory.computers[0].node,
  });
  expect(view.core.calls.some(call => call.method === 'sessions.close')).toBe(
    false,
  );
  await act(() => ctx.disconnect());
  // R02：已连接时断开要先确认。
  await confirmDisconnect();
  expect(view.core.calls).toContainEqual({
    method: 'sessions.close',
    args: ['s1'],
  });
  expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('records phase and first-frame times and passes reduced motion to the controller', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isReduceMotionEnabled')
    .mockResolvedValue(true);
  let ctx: SessionController | undefined;
  const navigation = mockNavigation<'Session'>();
  function Probe() {
    ctx = useSessionController({
      navigation,
      route: mockRoute('Session', { hostId }),
    });
    return null;
  }
  const view = await renderWithProviders(<Probe />, {
    core: createFakeCore({ sessions: [] }),
  });
  clients.push(view.queryClient);
  await advance();
  const start = ctx?.startedAtMs;
  await advance(100);
  await state(view.core, 'streaming');
  const streaming = ctx?.phaseAtMs.streaming;
  expect(streaming).toBeGreaterThan(start ?? 0);
  await advance(100);
  await act(() => view.core.emit(stats));
  expect(ctx?.firstFrameAtMs).toBeNull();
  await act(() => ctx?.onFirstFrame());
  const firstFrame = ctx?.firstFrameAtMs;
  expect(firstFrame).toBeGreaterThan(streaming ?? 0);
  await advance(100);
  await act(() => ctx?.onFirstFrame());
  expect(ctx?.firstFrameAtMs).toBe(firstFrame);
  await state(view.core, 'streaming');
  expect(ctx?.phaseAtMs.streaming).toBe(streaming);
  await state(view.core, 'reconnecting');
  await advance(100);
  await state(view.core, 'streaming');
  expect(ctx?.phaseAtMs.streaming).toBeGreaterThan(streaming ?? 0);
  expect(ctx?.reduceMotion).toBe(true);
});

describe('session switcher (RS-MULTI1)', () => {
  const windows: RootStackParamList['AdapterSession'] = {
    target: { protocol: 'rdp', hostId: 'h2', addr: '100.64.0.9' },
    name: 'Win PC',
    addrKind: 'tailnet',
  };

  /** A Remote Desktop session folded away before this page opened. */
  function foldedWindows(): void {
    const startedAtUs = Date.now() * 1000 - 60e6;
    adapterLive.put({
      sessionId: 'a2',
      params: windows,
      name: 'Win PC',
      phase: 'streaming',
      error: null,
      screens: [],
      screenIndex: 0,
      mode: 'trackpad',
      startedAtUs,
      collapsedAtUs: startedAtUs,
      history: {
        live: null,
        stoppedAtUs: null,
        error: null,
        name: 'Win PC',
        targetId: 'host:h2',
      },
    });
  }

  function closed(core: FakeCore) {
    return core.calls.filter(call => call.method === 'sessions.close');
  }

  afterEach(() => {
    resetAdapterLive();
    resetArrival();
    mockAnimateSpace.mockClear();
  });

  it('the toolbar opens the switcher with this session marked', async () => {
    const view = await mount();
    await stream(view.core);

    await fireEvent.press(view.getByTestId('toolbar-switch'));
    expect(view.getByTestId('session-switcher')).toBeOnTheScreen();
    expect(view.getByTestId('switcher-item-s1')).toBeSelected();
    expect(view.getAllByTestId(/^switcher-item-/)).toHaveLength(1);
    expect(view.getByTestId('switcher-other')).toBeOnTheScreen();
  });

  it('folds this session away and replaces the page with the tapped one', async () => {
    foldedWindows();
    const view = await mount();
    await stream(view.core);
    await fireEvent.press(view.getByTestId('toolbar-switch'));

    await fireEvent.press(view.getByTestId('switcher-item-a2'));
    expect(view.navigation.replace).toHaveBeenCalledWith('AdapterSession', {
      ...windows,
      sessionId: 'a2',
    });
    expect(view.navigation.navigate).not.toHaveBeenCalled();
    await cleanup();
    expect(closed(view.core)).toEqual([]);
  });

  it('连接其他电脑 folds this session away and opens the computer list', async () => {
    const view = await mount();
    await stream(view.core);
    await fireEvent.press(view.getByTestId('toolbar-switch'));

    await fireEvent.press(view.getByTestId('switcher-other'));
    expect(view.navigation.popTo).toHaveBeenCalledWith('MainTabs', {
      screen: 'Home',
    });
    expect(view.navigation.replace).not.toHaveBeenCalled();
    await cleanup();
    expect(closed(view.core)).toEqual([]);
  });

  it('tapping the current session closes the switcher and leaves the page working (RS-SPACES1)', async () => {
    const view = await mount();
    await stream(view.core);
    await fireEvent.press(view.getByTestId('toolbar-switch'));
    const sheet = view.getByTestId('session-switcher');
    // Drawn in the page's sheet layer, not in a native Modal over the page.
    const layer = within(view.getByTestId('layer-sheet'));
    expect(layer.getByTestId('session-switcher')).toBe(sheet);
    const modals = hostTypesAbove(sheet).filter(type => /modal/i.test(type));
    expect(modals).toEqual([]);

    await fireEvent.press(view.getByTestId('switcher-item-s1'));
    expect(view.queryByTestId('session-switcher')).toBeNull();
    expect(view.queryByTestId('layer-sheet')).toBeNull();
    expect(view.navigation.replace).not.toHaveBeenCalled();
    expect(view.navigation.navigate).not.toHaveBeenCalled();
    expect(closed(view.core)).toEqual([]);

    // The toolbar still answers, and X closes the reopened switcher.
    await fireEvent.press(view.getByTestId('toolbar-switch'));
    expect(view.getByTestId('session-switcher')).toBeOnTheScreen();
    await fireEvent.press(view.getByTestId('session-switcher-close'));
    expect(view.queryByTestId('session-switcher')).toBeNull();
    // So does tapping current again after reopening, then the backdrop.
    await fireEvent.press(view.getByTestId('toolbar-switch'));
    await fireEvent.press(view.getByTestId('switcher-item-s1'));
    expect(view.queryByTestId('session-switcher')).toBeNull();
    await fireEvent.press(view.getByTestId('toolbar-switch'));
    await fireEvent.press(view.getByTestId('session-switcher-backdrop'));
    expect(view.queryByTestId('session-switcher')).toBeNull();
    expect(view.navigation.replace).not.toHaveBeenCalled();
  });

  it('switches with ⌃⌥⌘ and ignores numbers past the list', async () => {
    foldedWindows();
    const view = await mount();
    await stream(view.core);
    const shortcut = keyboardProps().onShortcut;
    expect(shortcut).toBeDefined();

    // a2 started first, this session (s1) is second.
    await act(async () => shortcut?.({ action: 'index', index: 2 }));
    await act(async () => shortcut?.({ action: 'index', index: 3 }));
    expect(view.navigation.replace).not.toHaveBeenCalled();

    await act(async () => shortcut?.({ action: 'switcher' }));
    expect(view.getByTestId('session-switcher')).toBeOnTheScreen();

    await act(async () => shortcut?.({ action: 'previous' }));
    expect(view.navigation.replace).toHaveBeenCalledWith('AdapterSession', {
      ...windows,
      sessionId: 'a2',
    });
    expect(view.queryByTestId('session-switcher')).toBeNull();
  });

  it('⌃⌥⌘← plays the swipe slide toward the target before switching (RS-SPACES1)', async () => {
    foldedWindows();
    const view = await mount();
    await stream(view.core);

    await act(async () => keyboardProps().onShortcut?.({ action: 'previous' }));
    expect(mockAnimateSpace).toHaveBeenCalledWith('a2', 'previous');
    expect(view.navigation.replace).toHaveBeenCalledTimes(1);
    expect(mockAnimateSpace.mock.invocationCallOrder[0]).toBeLessThan(
      jest.mocked(view.navigation.replace).mock.invocationCallOrder[0],
    );
  });

  it('a three-finger swipe shows the dots and switches once the slide ends (RS-SPACES1)', async () => {
    foldedWindows();
    const view = await mount();
    await stream(view.core);
    const video = view.getByTestId('remote-session-view-placeholder');
    expect(view.queryByTestId('space-indicator')).toBeNull();

    await fireEvent(video, 'spaceSwipe', { phase: 'began' });
    expect(view.getByTestId('space-indicator')).toBeOnTheScreen();
    expect(view.getAllByTestId(/^space-dot-/)).toHaveLength(2);
    // This session started second.
    expect(view.getByTestId('space-dot-1')).toBeSelected();

    await fireEvent(video, 'spaceSwipe', { phase: 'ended' });
    await fireEvent(video, 'spaceSwitch', { sessionId: 'a2' });
    expect(view.navigation.replace).toHaveBeenCalledTimes(1);
    expect(view.navigation.replace).toHaveBeenCalledWith('AdapterSession', {
      ...windows,
      sessionId: 'a2',
    });
    // The page it opens knows it was switched to.
    expect(peekArrival('a2')).toBe(true);
    // Folded away, not ended.
    await cleanup();
    expect(closed(view.core)).toEqual([]);
  });

  it('a page switched to opens on the last picture until its first frame (RS-SPACES1)', async () => {
    const core = createFakeCore({ sessions: [liveSession] });
    markArrival(liveSession.id);
    const view = await mount({ sessionId: liveSession.id }, { core });
    const video = view.getByTestId('remote-session-view-placeholder');

    // No "connecting" over the picture: the native view shows the snapshot.
    expect(video).toHaveProp('showSnapshot', true);
    expect(view.queryByTestId('layer-connecting')).toBeNull();
    expect(view.getByTestId('space-indicator')).toBeOnTheScreen();

    await fireEvent(video, 'onFirstFrame');
    expect(video).toHaveProp('showSnapshot', false);
    expect(view.queryByTestId('layer-connecting')).toBeNull();
    // The dots fade a moment later.
    await advance(2000);
    expect(view.queryByTestId('space-indicator')).toBeNull();
  });

  it('a switched-to page that gets no frame falls back to connecting after 3 s (RS-SPACES1)', async () => {
    const core = createFakeCore({ sessions: [liveSession] });
    markArrival(liveSession.id);
    const view = await mount({ sessionId: liveSession.id }, { core });
    expect(view.queryByTestId('layer-connecting')).toBeNull();

    await advance(3000);
    expect(view.getByTestId('remote-session-view-placeholder')).toHaveProp(
      'showSnapshot',
      false,
    );
    expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
  });

  it('a page opened from the list keeps the plain connecting layer (RS-SPACES1)', async () => {
    const core = createFakeCore({ sessions: [liveSession] });
    const view = await mount({ sessionId: liveSession.id }, { core });
    expect(view.getByTestId('remote-session-view-placeholder')).toHaveProp(
      'showSnapshot',
      false,
    );
    expect(view.getByTestId('layer-connecting')).toBeOnTheScreen();
    expect(view.queryByTestId('space-indicator')).toBeNull();
  });
});
