import { act, cleanup, fireEvent, render } from '@testing-library/react-native';
import { AccessibilityInfo, Animated, StyleSheet } from 'react-native';
import type { RefObject } from 'react';
import { setViewport, videoStore } from '../../features/session/videoStore';
import { liveSession } from '../../native/rscore/fixtures';
import type { SessionRecord } from '../../features/session/store';
import type { SessionDisplay } from '../../native/rscore/types';
import type { RemoteSessionHandle } from '../../native/session/RemoteSessionView';
import SessionMinimap from './SessionMinimap';
import type { SessionCtx } from './slots';

const displays: SessionDisplay[] = [
  {
    id: 1,
    name: 'Studio Display',
    widthPx: 2560,
    heightPx: 1600,
    primary: true,
  },
  {
    id: 2,
    name: '竖屏显示器',
    widthPx: 2160,
    heightPx: 3840,
    primary: false,
  },
];

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays,
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
    ...overrides,
  };
}

function context(
  overrides: Partial<SessionCtx> = {},
  zoomReset = jest.fn(),
): SessionCtx {
  const rec = overrides.rec ?? record();
  const videoRef: RefObject<RemoteSessionHandle | null> = {
    current: {
      zoomReset,
      centerCursor: jest.fn(),
      showMagnifier: jest.fn(),
      animateSpace: jest.fn(),
    },
  };
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    sessionId: rec.id,
    videoRef,
    rec,
    view: {
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
        displayLabel: '1/2',
        displayA11y: '切换显示器，当前 1 / 2',
        qualityLabel: '自动',
        qualityA11y: '画质：自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: 6,
      fps: 60,
      tcp: false,
      alertText: null,
      a11y: '连接质量：直连 6 毫秒，60 帧',
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
    ...overrides,
  };
}

async function mount(ctx: SessionCtx) {
  const view = await render(<SessionMinimap ctx={ctx} />);
  await act(async () => {});
  return { ...view, zoomReset: ctx.videoRef.current?.zoomReset };
}

beforeEach(() => {
  videoStore.setState({ stats: {}, viewport: {} });
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockImplementation(() => Promise.resolve(false));
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  await act(async () => {});
});

it('stays hidden at 1× and appears with the centered location at 2×', async () => {
  const view = await mount(context());
  expect(view.queryByTestId('session-minimap')).toBeNull();

  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0.25, y: 0.25 }));
  expect(view.getByTestId('session-minimap')).toBeOnTheScreen();
  expect(
    view.getByLabelText('画面位置：正在看显示器 1 的中间部分'),
  ).toBeOnTheScreen();
});

it('labels the current display when the host has more than one', async () => {
  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0, y: 0 }));
  const view = await mount(context({ rec: record({ displayId: 2 }) }));
  expect(
    view.getByLabelText('画面位置：正在看显示器 2 的左上部分'),
  ).toBeOnTheScreen();
});

it('uses the canvas frame and white viewport outline', async () => {
  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0.25, y: 0.25 }));
  const view = await mount(context());
  expect(view.getByTestId('session-minimap')).toHaveStyle({
    width: 98,
    height: 63,
    borderRadius: 8,
    backgroundColor: '#0A0D10',
    borderWidth: 1,
  });
  expect(view.getByTestId('session-minimap-viewport')).toHaveStyle({
    left: '25%',
    top: '25%',
    width: '50%',
    height: '50%',
    borderWidth: 1.5,
    borderColor: '#FFFFFF',
  });
});

it('resets zoom when tapped', async () => {
  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0.25, y: 0.25 }));
  const zoomReset = jest.fn();
  const view = await mount(context({}, zoomReset));
  await fireEvent.press(view.getByTestId('session-minimap'));
  expect(zoomReset).toHaveBeenCalledTimes(1);
});

it('hides while the keyboard is open or another overlay is active', async () => {
  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0.25, y: 0.25 }));
  const keyboard = await mount(context({ keyboardOpen: true }));
  expect(keyboard.queryByTestId('session-minimap')).toBeNull();
  await keyboard.unmount();

  const overlay = await mount(
    context({
      view: {
        ...context().view,
        overlay: 'reconnecting',
      },
    }),
  );
  expect(overlay.queryByTestId('session-minimap')).toBeNull();
});

it('hides from the accessibility tree when the screen reader is on', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(true);
  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0.25, y: 0.25 }));
  const view = await mount(context());
  expect(view.queryByTestId('session-minimap')).toBeNull();
});

it('does not animate when reduced motion is enabled', async () => {
  const timing = jest.spyOn(Animated, 'timing');
  await act(() => setViewport(liveSession.id, { zoom: 2, x: 0.25, y: 0.25 }));
  const view = await mount(context({ reduceMotion: true }));
  expect(view.getByTestId('session-minimap')).toBeOnTheScreen();
  expect(timing).not.toHaveBeenCalled();
  expect(
    StyleSheet.flatten(view.getByTestId('session-minimap-screen').props.style)
      .backgroundColor,
  ).toBe('#161B1F');
});
