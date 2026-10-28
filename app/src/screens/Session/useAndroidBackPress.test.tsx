import { cleanup, renderHook } from '@testing-library/react-native';
import { BackHandler, Platform } from 'react-native';
import type { SessionView } from '../../features/session/view';
import type { SessionId } from '../../native/rscore/types';
import { useAndroidBackPress } from './useAndroidBackPress';
import type { SessionController } from './useSessionController';

type BackPressHandler = Parameters<typeof BackHandler.addEventListener>[1];

const baseView: SessionView = {
  overlay: 'none',
  end: { kind: 'stay' },
  role: 'controller',
  inputBlocked: false,
  shared: null,
  localUse: false,
  relay: false,
  tcp: false,
  idle: false,
  toolbar: {
    displayLabel: null,
    displayA11y: null,
    qualityLabel: '自动',
    qualityA11y: '画质：自动',
    showCtrlAltDel: false,
    showModeSwitch: true,
    keyboardEnabled: true,
  },
};

function context(
  overrides: Partial<SessionController> = {},
): SessionController {
  return {
    hostId: 'host-1',
    hostName: 'Studio Mac mini',
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    sessionId: 'session-1' as SessionId,
    videoRef: { current: null },
    rec: undefined,
    view: baseView,
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: 6,
      fps: 60,
      tcp: false,
      alertText: null,
      a11y: '连接质量：直连 6 毫秒，60 帧，查看详情',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: 0,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect: jest.fn(),
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
    sheet: null,
    coachVisible: false,
    toast: null,
    hideToast: jest.fn(),
    closeKeyboard: jest.fn(),
    keyboardInset: 0,
    onGesture: jest.fn(),
    onFirstFrame: jest.fn(),
    disconnectPrompt: null,
    confirmDisconnect: jest.fn(),
    dismissDisconnect: jest.fn(),
    showToolbar: jest.fn(),
    retryConnect: jest.fn(),
    switcherList: { screens: [], terminals: [] },
    switchTo: jest.fn(),
    connectOther: jest.fn(),
    onShortcut: jest.fn(),
    spaces: {
      spaces: [],
      showSnapshot: false,
      indicator: null,
      onSpaceSwipe: jest.fn(),
      onSpaceSwitch: jest.fn(),
      slideTo: jest.fn(),
    },
    elsewhereIds: [],
    inMainWindow: true,
    ...overrides,
  };
}

function captureBackPress() {
  let handler: BackPressHandler | null = null;
  const remove = jest.fn();
  const addEventListener = jest
    .spyOn(BackHandler, 'addEventListener')
    .mockImplementation((eventName, listener) => {
      if (eventName === 'hardwareBackPress') {
        handler = listener;
      }
      return { remove };
    });

  return {
    addEventListener,
    remove,
    press(): boolean | undefined {
      if (handler === null) {
        throw new Error('hardwareBackPress listener was not registered');
      }
      return handler({ type: 'hardwareBackPress', timeStamp: 0 });
    },
  };
}

async function mount(value: SessionController = context()) {
  const back = captureBackPress();
  const view = await renderHook(() => useAndroidBackPress(value));
  return { ...view, ...back, ctx: value };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('does not register a listener on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const addEventListener = jest.spyOn(BackHandler, 'addEventListener');

  await renderHook(() => useAndroidBackPress(context()));

  expect(addEventListener).not.toHaveBeenCalled();
});

it('warns on the first Android back press without hiding the toolbar', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  jest.spyOn(Date, 'now').mockReturnValue(1_000);
  const { ctx, press } = await mount();

  expect(press()).toBe(true);
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).toHaveBeenCalledWith(
    '再按一次返回就收起会话，连接保持',
  );
  expect(ctx.collapse).not.toHaveBeenCalled();
});

it('keeps the toolbar hidden but still warns on the first back press', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  jest.spyOn(Date, 'now').mockReturnValue(1_000);
  const { ctx, press } = await mount(context({ toolbarHidden: true }));

  expect(press()).toBe(true);
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).toHaveBeenCalledWith(
    '再按一次返回就收起会话，连接保持',
  );
  expect(ctx.collapse).not.toHaveBeenCalled();
});

it('returns to the sessions list without disconnecting on a second back press', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  let now = 1_000;
  jest.spyOn(Date, 'now').mockImplementation(() => now);
  const { ctx, press } = await mount();

  expect(press()).toBe(true);
  now = 3_000;
  expect(press()).toBe(true);

  expect(ctx.collapse).toHaveBeenCalledTimes(1);
  expect(ctx.disconnect).not.toHaveBeenCalled();
});

it('treats a back press after the three-second window as a new first press', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  let now = 1_000;
  jest.spyOn(Date, 'now').mockImplementation(() => now);
  const { ctx, press } = await mount();

  expect(press()).toBe(true);
  now = 4_500;
  expect(press()).toBe(true);

  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).toHaveBeenCalledTimes(2);
  expect(ctx.collapse).not.toHaveBeenCalled();
});

it('closes a sheet before handling any other back action', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const { ctx, press } = await mount(
    context({
      sheet: 'more',
      keyboardOpen: true,
      coachVisible: true,
      view: { ...baseView, overlay: 'connecting' },
    }),
  );

  expect(press()).toBe(true);
  expect(ctx.closeSheet).toHaveBeenCalledTimes(1);
  expect(ctx.closeKeyboard).not.toHaveBeenCalled();
  expect(ctx.closeCoach).not.toHaveBeenCalled();
  expect(ctx.collapse).not.toHaveBeenCalled();
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).not.toHaveBeenCalled();
});

it('closes the keyboard before the coach or toolbar', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const { ctx, press } = await mount(
    context({ keyboardOpen: true, coachVisible: true }),
  );

  expect(press()).toBe(true);
  expect(ctx.closeKeyboard).toHaveBeenCalledTimes(1);
  expect(ctx.closeSheet).not.toHaveBeenCalled();
  expect(ctx.closeCoach).not.toHaveBeenCalled();
  expect(ctx.collapse).not.toHaveBeenCalled();
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).not.toHaveBeenCalled();
});

it('closes the gesture coach before collapsing the toolbar', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const { ctx, press } = await mount(context({ coachVisible: true }));

  expect(press()).toBe(true);
  expect(ctx.closeCoach).toHaveBeenCalledTimes(1);
  expect(ctx.closeSheet).not.toHaveBeenCalled();
  expect(ctx.closeKeyboard).not.toHaveBeenCalled();
  expect(ctx.collapse).not.toHaveBeenCalled();
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).not.toHaveBeenCalled();
});

it.each(['opening', 'connecting', 'approval'] as const)(
  'cancels the connection on back while %s',
  async overlay => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const { ctx, press } = await mount(
      context({ view: { ...baseView, overlay } }),
    );

    expect(press()).toBe(true);
    expect(ctx.cancelConnect).toHaveBeenCalledTimes(1);
    expect(ctx.collapse).not.toHaveBeenCalled();
    expect(ctx.disconnect).not.toHaveBeenCalled();
    expect(ctx.showToast).not.toHaveBeenCalled();
  },
);

it('cancels the connection on back while the first frame is still missing', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const { ctx, press } = await mount(
    context({
      rec: { phase: 'streaming' } as SessionController['rec'],
      firstFrameAtMs: null,
    }),
  );

  expect(press()).toBe(true);
  expect(ctx.cancelConnect).toHaveBeenCalledTimes(1);
  expect(ctx.collapse).not.toHaveBeenCalled();
});

it('lets the navigation handle back while reconnecting', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const { ctx, press } = await mount(
    context({ view: { ...baseView, overlay: 'reconnecting' } }),
  );

  expect(press()).toBe(false);
  expect(ctx.cancelConnect).not.toHaveBeenCalled();
  expect(ctx.closeSheet).not.toHaveBeenCalled();
  expect(ctx.closeKeyboard).not.toHaveBeenCalled();
  expect(ctx.closeCoach).not.toHaveBeenCalled();
  expect(ctx.collapse).not.toHaveBeenCalled();
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).not.toHaveBeenCalled();
  expect(ctx.disconnect).not.toHaveBeenCalled();
});

it('removes the listener on unmount', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const { remove, unmount } = await mount();

  await unmount();

  expect(remove).toHaveBeenCalledTimes(1);
});

it('clears the first-press state when the session changes', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  let now = 1_000;
  jest.spyOn(Date, 'now').mockImplementation(() => now);
  const first = context();
  const back = captureBackPress();
  const view = await renderHook<void, { value: SessionController }>(
    props => useAndroidBackPress(props.value),
    {
      initialProps: { value: first },
    },
  );

  expect(back.press()).toBe(true);
  now = 2_000;
  await view.rerender({
    value: { ...first, sessionId: 'session-2' as SessionId },
  });
  expect(back.press()).toBe(true);

  expect(first.collapse).not.toHaveBeenCalled();
  expect(first.showToast).toHaveBeenCalledTimes(2);
  expect(back.addEventListener).toHaveBeenCalledTimes(1);
});
