import { act, cleanup, fireEvent, within } from '@testing-library/react-native';
import { AccessibilityInfo, StyleSheet } from 'react-native';
import { hudModel } from '../../features/session/hud';
import type { SessionRecord } from '../../features/session/store';
import { setVideoStats, videoStore } from '../../features/session/videoStore';
import {
  resetRecordingUiForTests,
  setRecordingUi,
} from '../../features/recording/sessionRecording';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import type { RemoteVideoStats } from '../../native/session/RemoteSessionView';
import { renderWithProviders } from '../../test/render';
import { SessionHud } from './SessionHud';
import type { SessionCtx } from './slots';

const stats: CoreEventOf<'session.stats'> = {
  type: 'session.stats',
  sessionId: liveSession.id,
  rttMs: 6,
  fps: 60,
  bitrateKbps: 18400,
  decodeMs: 2.1,
  lossPct: 0,
  width: 2560,
  height: 1600,
  codec: 'hevc',
  path: 'tailnetDirect',
  transport: 'quic',
};

const nativeStats: RemoteVideoStats = {
  fps: 59.6,
  decodeP50Ms: 4.2,
  decodeP95Ms: 8.7,
  latencyP50Ms: -1,
  queue: 0,
  dropped: 0,
  keyframeRequests: 0,
  width: 2560,
  height: 1600,
  codec: 'hevc',
};

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats,
    displays: [],
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

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  const rec = overrides.rec ?? record();
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
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
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '自动',
        qualityA11y: '画质：自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: hudModel(rec),
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

async function mount(ctx: SessionCtx, settings?: Record<string, unknown>) {
  const view = await renderWithProviders(<SessionHud ctx={ctx} />, {
    settings,
  });
  return view;
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(0);
  videoStore.setState({ stats: {}, viewport: {} });
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(false);
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('shows a direct-path pill and opens quality settings', async () => {
  const ctx = context();
  const view = await mount(ctx);
  expect(
    view.getByLabelText('连接质量：直连 6 毫秒，60 帧，查看详情'),
  ).toBeOnTheScreen();
  expect(view.getByText('直连')).toBeOnTheScreen();
  await fireEvent.press(view.getByTestId('session-hud-pill'));
  expect(ctx.openSheet).toHaveBeenCalledWith('quality');
  await advance(3000);
  expect(view.queryByText('直连')).toBeNull();
});

it('frames the relay pill with the amber 40% outline', async () => {
  const ctx = context({
    rec: record({
      path: 'tailnetDerp',
      stats: { ...stats, path: 'tailnetDerp' },
    }),
  });
  const view = await mount(ctx);
  const frame = view.getByTestId('session-hud-pill').parent;
  expect(StyleSheet.flatten(frame?.props.style)).toMatchObject({
    borderRadius: 16,
    borderWidth: 1,
    borderColor: 'rgba(244,178,78,0.4)',
  });
});

it('renders the four-row stats overlay when session.hud is enabled', async () => {
  const view = await mount(context(), { 'session.hud': true });
  expect(view.queryByTestId('session-hud-pill')).toBeNull();
  expect(view.getByText('延迟 6 ms')).toBeOnTheScreen();
  expect(view.getByText('帧率 60')).toBeOnTheScreen();
  expect(view.getByText('码率 18.4 Mbps')).toBeOnTheScreen();
  expect(view.getByText('解码 2.1 ms')).toBeOnTheScreen();
  expect(
    view.getByLabelText(
      '统计浮层：延迟 6 毫秒，60 帧，码率 18.4 Mbps，解码 2.1 毫秒',
    ),
  ).toBeOnTheScreen();
  expect(view.getByTestId('session-stats')).toHaveStyle({
    width: 168,
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 12,
  });
  expect(view.getByText('延迟 6 ms')).toHaveStyle({
    fontSize: 11,
    lineHeight: 17,
    color: '#D5DCE0',
  });
});

it('prefers fresh native video stats in the stats overlay', async () => {
  await act(() => setVideoStats(liveSession.id, nativeStats));
  const view = await mount(
    context({ rec: record({ stats: { ...stats, fps: 30 } }) }),
    { 'session.hud': true },
  );
  expect(view.getByText('帧率 60')).toBeOnTheScreen();
  expect(view.getByText('解码 4/9 ms')).toBeOnTheScreen();
  expect(
    view.getByLabelText(
      '统计浮层：延迟 6 毫秒，60 帧，码率 18.4 Mbps，解码中位 4 毫秒，95 分位 9 毫秒',
    ),
  ).toBeOnTheScreen();
});

it('falls back to Core stats when native video stats stop updating', async () => {
  await act(() => setVideoStats(liveSession.id, nativeStats));
  const view = await mount(context(), { 'session.hud': true });
  expect(view.getByText('解码 4/9 ms')).toBeOnTheScreen();
  await advance(4000);
  expect(view.getByText('帧率 60')).toBeOnTheScreen();
  expect(view.getByText('解码 2.1 ms')).toBeOnTheScreen();
});

it('keeps refreshed native video stats live', async () => {
  const latestStats = { ...nativeStats, fps: 30, decodeP50Ms: 7.4 };
  await act(() => setVideoStats(liveSession.id, nativeStats));
  const view = await mount(context(), { 'session.hud': true });
  await advance(2900);
  await act(() => setVideoStats(liveSession.id, latestStats));
  expect(view.getByText('解码 7/9 ms')).toBeOnTheScreen();
  await advance(3000);
  expect(view.getByText('解码 7/9 ms')).toBeOnTheScreen();
  await advance(1);
  expect(view.getByText('解码 2.1 ms')).toBeOnTheScreen();
});

it('passes fresh native fps to the quality pill', async () => {
  await act(() => setVideoStats(liveSession.id, nativeStats));
  const view = await mount(
    context({ rec: record({ stats: { ...stats, fps: 30 } }) }),
  );
  expect(view.getByText('60 fps')).toBeOnTheScreen();
  expect(view.queryByText('30 fps')).toBeNull();
});

it('opens the quality sheet from the stats overlay', async () => {
  const ctx = context();
  const view = await mount(ctx, { 'session.hud': true });
  await fireEvent.press(view.getByTestId('session-stats'));
  expect(ctx.openSheet).toHaveBeenCalledWith('quality');
});

it('shows the clipboard tip for a new from-host entry and hides it after 3 s', async () => {
  const initial = record();
  const view = await mount(context({ rec: initial }));
  expect(view.queryByText(/已同步剪贴板/)).toBeNull();
  const updated = record({
    clipboard: [{ direction: 'fromHost', text: 'a'.repeat(128), at: 1 }],
  });
  await view.rerender(<SessionHud ctx={context({ rec: updated })} />);
  expect(view.getByText('已同步剪贴板 · 128 字')).toBeOnTheScreen();
  expect(view.getByTestId('session-tip')).toHaveStyle({
    height: 32,
    borderRadius: 16,
    paddingHorizontal: 12,
  });
  expect(view.getByTestId('session-tip')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
  await advance(3000);
  expect(view.queryByText('已同步剪贴板 · 128 字')).toBeNull();
});

it('does not claim the clipboard synced while auto sync is off', async () => {
  const view = await mount(context({ rec: record() }), {
    'clipboard.autoSync': false,
  });
  const updated = record({
    clipboard: [{ direction: 'fromHost', text: 'hello', at: 1 }],
  });
  await view.rerender(<SessionHud ctx={context({ rec: updated })} />);
  expect(view.queryByText(/已同步剪贴板/)).toBeNull();
  expect(
    view.getByText('电脑上复制了 5 字，打开剪贴板面板可以取用'),
  ).toBeOnTheScreen();
});

it('does not announce a clipboard entry present on first render', async () => {
  const rec = record({
    clipboard: [{ direction: 'fromHost', text: 'hello', at: 1 }],
  });
  const view = await mount(context({ rec }));
  expect(view.queryByText(/已同步剪贴板/)).toBeNull();
});

it('shows a touch-mode tip after the mode changes', async () => {
  const view = await mount(context({ mode: 'trackpad' }));
  await view.rerender(<SessionHud ctx={context({ mode: 'touch' })} />);
  expect(view.getByText('触屏模式：点哪里就点哪里')).toBeOnTheScreen();
  await advance(3000);
  expect(view.queryByText('触屏模式：点哪里就点哪里')).toBeNull();
});

it('shows a trackpad-mode tip after switching back', async () => {
  const view = await mount(context({ mode: 'touch' }));
  await view.rerender(<SessionHud ctx={context({ mode: 'trackpad' })} />);
  expect(view.getByText('触控板模式：整块屏幕都是触控板')).toBeOnTheScreen();
});

it('uses em dashes when stats are unavailable', async () => {
  const view = await mount(context({ rec: record({ stats: null }) }), {
    'session.hud': true,
  });
  expect(view.getByText('延迟 —')).toBeOnTheScreen();
  expect(view.getByText('帧率 —')).toBeOnTheScreen();
  expect(view.getByText('码率 —')).toBeOnTheScreen();
  expect(view.getByText('解码 —')).toBeOnTheScreen();
});

it('hides the HUD from assistive technology while a screen reader is on', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(true);
  const addListener = jest.spyOn(AccessibilityInfo, 'addEventListener');
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockClear();
  const view = await mount(context(), { 'session.hud': true });
  const hudLayer = view.getByTestId('session-stats', {
    includeHiddenElements: true,
  }).parent;
  expect(hudLayer).toHaveProp('accessibilityElementsHidden', true);
  expect(hudLayer).toHaveProp(
    'importantForAccessibility',
    'no-hide-descendants',
  );
  expect(view.queryByTestId('session-stats')).toBeNull();
  expect(addListener).toHaveBeenCalledWith(
    'screenReaderChanged',
    expect.any(Function),
  );

  await view.rerender(<SessionHud ctx={context({ mode: 'touch' })} />);
  expect(announce).toHaveBeenCalledWith('触屏模式：点哪里就点哪里');
  const tipLayer = view.getByTestId('session-tip', {
    includeHiddenElements: true,
  }).parent;
  expect(tipLayer).toHaveProp('accessibilityElementsHidden', true);
  expect(tipLayer).toHaveProp(
    'importantForAccessibility',
    'no-hide-descendants',
  );

  await advance(3000);
  await view.rerender(<SessionHud ctx={context({ mode: 'trackpad' })} />);
  expect(announce).toHaveBeenCalledTimes(2);
  expect(announce).toHaveBeenLastCalledWith('触控板模式：整块屏幕都是触控板');
});

it('does not announce tips while a screen reader is off', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(false);
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockClear();
  const view = await mount(context({ mode: 'trackpad' }));
  await view.rerender(<SessionHud ctx={context({ mode: 'touch' })} />);
  expect(view.getByText('触屏模式：点哪里就点哪里')).toBeOnTheScreen();
  expect(announce).not.toHaveBeenCalled();
  expect(view.getByTestId('session-tip')).not.toHaveProp(
    'accessibilityElementsHidden',
  );
  expect(view.getByTestId('session-tip').parent).not.toHaveProp(
    'importantForAccessibility',
  );
});

it('puts the recording chip beside the HUD pill instead of on top of it', async () => {
  resetRecordingUiForTests();
  const core = createFakeCore();
  Object.assign(core, { recording: {} });
  setRecordingUi({ sessionId: liveSession.id });
  setRecordingUi({
    sessionId: liveSession.id,
    state: 'recording',
    durationMs: 12_000,
  });
  try {
    const view = await renderWithProviders(<SessionHud ctx={context()} />, {
      core,
    });
    const row = view.getByTestId('session-hud-row');
    expect(StyleSheet.flatten(row.props.style).flexDirection).toBe('row');
    expect(within(row).getByTestId('recording-chip')).toBeOnTheScreen();
    expect(within(row).getByTestId('session-hud-pill')).toBeOnTheScreen();
  } finally {
    resetRecordingUiForTests();
  }
});
