import { cleanup } from '@testing-library/react-native';
import { AccessibilityInfo } from 'react-native';
import { liveSession } from '../../native/rscore/fixtures';
import { mockWindowDimensions } from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import { LocalUseBar } from './LocalUseBar';
import type { SessionCtx } from './slots';

function context(localUse: boolean): SessionCtx {
  return {
    hostId: liveSession.hostId,
    hostName: liveSession.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: undefined,
    view: {
      overlay: 'none',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: localUse,
      shared: null,
      localUse,
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
        keyboardEnabled: !localUse,
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
  };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('shows both lines and announces the local-use state once', async () => {
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  const ctx = context(true);
  const view = await renderWithProviders(<LocalUseBar ctx={ctx} />);

  expect(
    view.getByText('对方正在使用这台电脑 · 你的输入先暂停'),
  ).toBeOnTheScreen();
  expect(view.getByText('对方停手 1 秒后自动交还')).toBeOnTheScreen();
  expect(announce).toHaveBeenCalledTimes(1);
  expect(announce).toHaveBeenCalledWith(
    '对方正在使用这台电脑 · 你的输入先暂停',
  );
  expect(view.getByTestId('local-use-bar')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
  expect(view.getByTestId('local-use-bar')).not.toHaveProp('accessibilityRole');
  view.queryClient.clear();
});

it('renders nothing when the local user is idle', async () => {
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  announce.mockClear();
  const view = await renderWithProviders(<LocalUseBar ctx={context(false)} />);

  expect(view.queryByTestId('local-use-bar')).toBeNull();
  expect(view.toJSON()).toBeNull();
  expect(announce).not.toHaveBeenCalled();
  view.queryClient.clear();
});

it('caps the bar at 396 and keeps 59 pt side insets when narrow', async () => {
  const wide = mockWindowDimensions({ width: 1000, height: 600 });
  try {
    const view = await renderWithProviders(
      <LocalUseBar ctx={context(true)} />,
    );
    expect(view.getByTestId('local-use-bar')).toHaveStyle({ width: 396 });
    view.queryClient.clear();
    await view.unmount();
  } finally {
    wide();
  }

  const narrow = mockWindowDimensions({ width: 360, height: 780 });
  try {
    const view = await renderWithProviders(
      <LocalUseBar ctx={context(true)} />,
    );
    expect(view.getByTestId('local-use-bar')).toHaveStyle({ width: 242 });
    view.queryClient.clear();
  } finally {
    narrow();
  }
});
