import { act, cleanup, renderHook } from '@testing-library/react-native';
import { Platform } from 'react-native';
import type { SessionView } from '../../features/session/view';
import { clearVideo, setVideoStats } from '../../features/session/videoStore';
import type { RemoteVideoStats } from '../../native/session/RemoteSessionView';
import type { SessionId } from '../../native/rscore/types';
import type { SessionRecord } from '../../features/session/store';
import type { PipChange } from '../../native/specs/NativeRsAndroidSession';
import { useAndroidPip } from './useAndroidPip';
import type { SessionController } from './useSessionController';

const mockSetPipEligible = jest.fn<void, [boolean, number, number]>();
let mockPipListener: ((event: PipChange) => void) | undefined;
const mockRemovePipListener = jest.fn();

jest.mock('../../native/androidSession/androidSession', () => ({
  setPipEligible: (eligible: boolean, width: number, height: number) =>
    mockSetPipEligible(eligible, width, height),
  subscribePipChange: (listener: (event: PipChange) => void) => {
    mockPipListener = listener;
    return mockRemovePipListener;
  },
}));

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

const streamingRecord = {
  phase: 'streaming',
} as SessionRecord;

const videoStats: RemoteVideoStats = {
  fps: 60,
  decodeP50Ms: 1,
  decodeP95Ms: 2,
  latencyP50Ms: 3,
  queue: 0,
  dropped: 0,
  keyframeRequests: 0,
  width: 1920,
  height: 1080,
  codec: 'h264',
};

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'android');
  mockSetPipEligible.mockClear();
  mockRemovePipListener.mockClear();
  mockPipListener = undefined;
  setVideoStats('session-1' as SessionId, videoStats);
});

afterEach(async () => {
  clearVideo('session-1' as SessionId);
  await cleanup();
  jest.restoreAllMocks();
});

it('does not call Android PiP APIs on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');

  await renderHook(() => useAndroidPip(context()));

  expect(mockSetPipEligible).not.toHaveBeenCalled();
  expect(mockPipListener).toBeUndefined();
});

it('marks a controller as ineligible while retaining the video dimensions', async () => {
  await renderHook(() => useAndroidPip(context()));

  expect(mockSetPipEligible).toHaveBeenCalledWith(false, 1920, 1080);
});

it('marks a streaming viewer with a video size as eligible', async () => {
  await renderHook(() =>
    useAndroidPip(
      context({ view: { ...baseView, role: 'viewer' }, rec: streamingRecord }),
    ),
  );

  expect(mockSetPipEligible).toHaveBeenCalledWith(true, 1920, 1080);
});

it('does not resend the same eligibility and dimensions on rerender', async () => {
  const rendered = await renderHook(() =>
    useAndroidPip(
      context({ view: { ...baseView, role: 'viewer' }, rec: streamingRecord }),
    ),
  );
  mockSetPipEligible.mockClear();

  await rendered.rerender(undefined);

  expect(mockSetPipEligible).not.toHaveBeenCalled();
});

it('closes transient UI and toggles the toolbar around PiP', async () => {
  const ctx = context({
    view: { ...baseView, role: 'viewer' },
    rec: streamingRecord,
    sheet: 'more',
    keyboardOpen: true,
  });
  const rendered = await renderHook(() => useAndroidPip(ctx));
  expect(mockPipListener).toBeDefined();

  await act(() => mockPipListener?.({ inPip: true }));
  expect(ctx.closeSheet).toHaveBeenCalledTimes(1);
  expect(ctx.closeKeyboard).toHaveBeenCalledTimes(1);
  expect(ctx.onGesture).toHaveBeenCalledTimes(1);
  expect(ctx.onGesture).toHaveBeenLastCalledWith({ kind: 'threeFingerTap' });

  await act(() => mockPipListener?.({ inPip: false }));
  expect(ctx.onGesture).toHaveBeenCalledTimes(2);
  await rendered.unmount();
});

it('leaves an already hidden toolbar unchanged', async () => {
  const ctx = context({
    view: { ...baseView, role: 'viewer' },
    rec: streamingRecord,
    toolbarHidden: true,
  });
  await renderHook(() => useAndroidPip(ctx));

  await act(() => mockPipListener?.({ inPip: true }));
  await act(() => mockPipListener?.({ inPip: false }));

  expect(ctx.onGesture).not.toHaveBeenCalled();
});

it('clears native eligibility when unmounted', async () => {
  const rendered = await renderHook(() => useAndroidPip(context()));
  mockSetPipEligible.mockClear();

  await rendered.unmount();

  expect(mockSetPipEligible).toHaveBeenCalledWith(false, 0, 0);
});
