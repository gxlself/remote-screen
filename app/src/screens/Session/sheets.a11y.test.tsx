import { act, fireEvent } from '@testing-library/react-native';
import { AccessibilityInfo, PixelRatio } from 'react-native';
import { computers, liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { mockWindowDimensions } from '../../test/dimensions';
import DisplaySheet from './DisplaySheet';
import GestureCoach from './GestureCoach';
import MoreSheet from './MoreSheet';
import ClipboardSheet from './ClipboardSheet';
import type { SessionCtx } from './slots';

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: computers[0].hostId,
    hostName: computers[0].name,
    os: 'macos',
    host: computers[0],
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: {
      ...liveSession,
      phase: 'streaming',
      role: 'controller',
      controller: null,
      viewers: [],
      inputPausedLocal: false,
      stats: null,
      displays: [
        {
          id: 1,
          name: '主显示器',
          widthPx: 1920,
          heightPx: 1080,
          primary: true,
          refreshHz: 60,
        },
      ],
      displayId: 1,
      dial: [],
      approvalTimeoutS: null,
      derpRegion: null,
      reconnectReason: null,
      idleWarning: null,
      controlRequest: null,
      lastError: null,
      clipboard: [],
    },
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
        qualityA11y: '画质自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '',
    },
    isPad: false,
    reduceMotion: true,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: null,
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

afterEach(() => {
  jest.clearAllTimers();
  jest.restoreAllMocks();
});

it('uses checked and selected radio state and closes from the escape action', async () => {
  const onClose = jest.fn();
  const view = await renderWithProviders(
    <DisplaySheet ctx={context()} onClose={onClose} />,
  );
  const radio = view.getByRole('radio');
  expect(radio.props.accessibilityState).toEqual({
    checked: true,
    selected: true,
  });
  fireEvent(view.getByTestId('display-sheet'), 'accessibilityEscape');
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('provides an escape action and screen reader note for GestureCoach', async () => {
  const onClose = jest.fn();
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(true);
  const view = await renderWithProviders(
    <GestureCoach ctx={context()} onClose={onClose} />,
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(
    view.getByText(
      '读屏开启时这些手势由读屏接管；可以用底部工具栏切换操作方式、打开键盘，在「更多」里还原缩放。',
    ),
  ).toBeOnTheScreen();
  fireEvent(view.getByTestId('gesture-coach'), 'accessibilityEscape');
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('keeps zoom reset usable for screen reader users', async () => {
  const zoomReset = jest.fn();
  const videoRef = {
    current: { zoomReset },
  } as unknown as SessionCtx['videoRef'];
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(true);
  const onClose = jest.fn();
  const view = await renderWithProviders(
    <MoreSheet ctx={context({ videoRef })} onClose={onClose} />,
  );
  await act(async () => {
    await Promise.resolve();
  });
  await fireEvent.press(view.getByLabelText('还原缩放'));
  expect(zoomReset).toHaveBeenCalledTimes(1);
  expect(onClose).toHaveBeenCalledTimes(1);

  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(false);
  const noReader = await renderWithProviders(
    <MoreSheet ctx={context({ videoRef })} onClose={jest.fn()} />,
  );
  // R22：没缩放时这一行仍在，只是不能点，并说明原因。
  expect(noReader.getByTestId('more-zoom-reset')).toBeDisabled();
});

it('caps ClipboardSheet height for a short landscape window', async () => {
  const restore = mockWindowDimensions({ width: 844, height: 390 });
  try {
    const view = await renderWithProviders(
      <ClipboardSheet ctx={context()} onClose={jest.fn()} />,
    );
    expect(view.getByTestId('clipboard-body')).toHaveStyle({ maxHeight: 258 });
  } finally {
    restore();
  }
});

it('uses natural grid rows at large font scale', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await renderWithProviders(
    <GestureCoach ctx={context()} onClose={jest.fn()} />,
  );
  expect(view.getByTestId('gesture-coach-scroll')).toBeOnTheScreen();
});
