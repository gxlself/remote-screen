import { act, fireEvent, render, screen } from '@testing-library/react-native';
import {
  AccessibilityInfo,
  Animated,
  AppState,
  Linking,
  Platform,
  UIManager,
} from 'react-native';
import type { AppStateStatus } from 'react-native';
import ScannerSlot from './ScannerSlot';

// Fabric's generated HostComponent is a string at runtime.
jest.mock('../../native/specs/RsQrScannerNativeComponent', () => ({
  __esModule: true,
  default: 'RsQrScannerView',
}));

const originalPlatform = Platform.OS;

beforeEach(() => {
  jest.clearAllMocks();
  Object.defineProperty(Platform, 'OS', { configurable: true, value: 'ios' });
  jest.spyOn(UIManager, 'hasViewManagerConfig').mockReturnValue(true);
});

afterEach(() => {
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: originalPlatform,
  });
  jest.restoreAllMocks();
});

it('forwards native codes only while active', async () => {
  const onScan = jest.fn();
  const rendered = await render(<ScannerSlot active onScan={onScan} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onCode', {
    nativeEvent: { text: 'rsp://pair?v=1' },
  });
  expect(onScan).toHaveBeenCalledWith('rsp://pair?v=1');
  await rendered.rerender(<ScannerSlot active={false} onScan={onScan} />);
  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('active', false);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onCode', {
    nativeEvent: { text: 'rsp://pair?v=2' },
  });
  expect(onScan).toHaveBeenCalledTimes(1);
});

it('shows a torch button when the running scanner reports a torch', async () => {
  await render(<ScannerSlot active onScan={jest.fn()} />);
  const native = screen.getByTestId('scanner-slot-native');
  await fireEvent(native, 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: true },
  });

  const button = screen.getByTestId('scanner-slot-torch');
  expect(button).toHaveProp('accessibilityLabel', '打开手电筒');
  await fireEvent.press(button);

  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('torch', true);
  expect(screen.getByTestId('scanner-slot-torch')).toHaveProp(
    'accessibilityLabel',
    '关闭手电筒',
  );
  await fireEvent.press(screen.getByTestId('scanner-slot-torch'));
  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('torch', false);
  expect(screen.getByTestId('scanner-slot-torch')).toHaveProp(
    'accessibilityLabel',
    '打开手电筒',
  );
});

it('clears the torch when scanning becomes inactive and resumes with it off', async () => {
  const onScan = jest.fn();
  const rendered = await render(<ScannerSlot active onScan={onScan} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: true },
  });
  await fireEvent.press(screen.getByTestId('scanner-slot-torch'));
  await rendered.rerender(<ScannerSlot active={false} onScan={onScan} />);
  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('torch', false);
  expect(screen.queryByTestId('scanner-slot-torch')).toBeNull();

  await rendered.rerender(<ScannerSlot active onScan={onScan} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: true },
  });
  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('torch', false);
  expect(screen.getByTestId('scanner-slot-torch')).toHaveProp(
    'accessibilityLabel',
    '打开手电筒',
  );
});

it('clears the torch in the background and removes its app-state listener on unmount', async () => {
  let changeAppState: ((state: AppStateStatus) => void) | undefined;
  const remove = jest.fn();
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_, handler) => {
    changeAppState = handler;
    return { remove };
  });
  const rendered = await render(<ScannerSlot active onScan={jest.fn()} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: true },
  });
  await fireEvent.press(screen.getByTestId('scanner-slot-torch'));

  await act(() => changeAppState?.('background'));
  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('torch', false);
  expect(screen.queryByTestId('scanner-slot-torch')).toBeNull();

  await act(() => changeAppState?.('active'));
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: true },
  });
  expect(screen.getByTestId('scanner-slot-native')).toHaveProp('torch', false);
  expect(screen.getByTestId('scanner-slot-torch')).toHaveProp(
    'accessibilityLabel',
    '打开手电筒',
  );
  await rendered.unmount();
  expect(remove).toHaveBeenCalledTimes(1);
});

it.each(['starting', 'error'])(
  'clears the torch when the native session reports %s',
  async state => {
    await render(<ScannerSlot active onScan={jest.fn()} />);
    await fireEvent(
      screen.getByTestId('scanner-slot-native'),
      'onScannerState',
      {
        nativeEvent: { state: 'running', message: '', hasTorch: true },
      },
    );
    await fireEvent.press(screen.getByTestId('scanner-slot-torch'));
    await fireEvent(
      screen.getByTestId('scanner-slot-native'),
      'onScannerState',
      {
        nativeEvent: { state, message: '', hasTorch: false },
      },
    );
    expect(screen.getByTestId('scanner-slot-native')).toHaveProp(
      'torch',
      false,
    );
    expect(screen.queryByTestId('scanner-slot-torch')).toBeNull();
  },
);

it('does not show a torch button when the running scanner has no torch', async () => {
  await render(<ScannerSlot active onScan={jest.fn()} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: false },
  });

  expect(screen.queryByTestId('scanner-slot-torch')).toBeNull();
});

it('does not start the scan-line animation when reduce motion is enabled', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isReduceMotionEnabled')
    .mockResolvedValueOnce(true);
  const loop = jest.spyOn(Animated, 'loop');
  try {
    await render(<ScannerSlot active onScan={jest.fn()} />);
    await fireEvent(
      screen.getByTestId('scanner-slot-native'),
      'onScannerState',
      {
        nativeEvent: { state: 'running', message: '', hasTorch: false },
      },
    );
    expect(loop).not.toHaveBeenCalled();
    expect(screen.getByTestId('scanner-slot-scan-line')).toHaveStyle({
      transform: [{ translateY: 68.8 }],
    });
  } finally {
    loop.mockRestore();
  }
});

it('stops an active scan-line loop when the reduce-motion preference changes', async () => {
  let changeReduceMotion: ((enabled: boolean) => void) | undefined;
  const remove = jest.fn();
  jest
    .spyOn(AccessibilityInfo, 'isReduceMotionEnabled')
    .mockResolvedValueOnce(false);
  jest
    .spyOn(AccessibilityInfo, 'addEventListener')
    .mockImplementation((_, handler) => {
      changeReduceMotion = handler;
      return { remove };
    });
  const start = jest.fn();
  const stop = jest.fn();
  jest.spyOn(Animated, 'loop').mockReturnValue({
    start,
    stop,
    reset: jest.fn(),
  });
  const rendered = await render(<ScannerSlot active onScan={jest.fn()} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'running', message: '', hasTorch: false },
  });
  expect(start).toHaveBeenCalledTimes(1);

  await act(() => changeReduceMotion?.(true));
  expect(stop).toHaveBeenCalledTimes(1);
  expect(screen.getByTestId('scanner-slot-scan-line')).toHaveStyle({
    transform: [{ translateY: 68.8 }],
  });
  await rendered.unmount();
  expect(remove).toHaveBeenCalledTimes(1);
});

it.each(['denied', 'restricted'])(
  'opens settings for %s permission',
  async state => {
    const openSettings = jest
      .spyOn(Linking, 'openSettings')
      .mockResolvedValue(undefined);
    await render(<ScannerSlot active onScan={jest.fn()} />);
    await fireEvent(
      screen.getByTestId('scanner-slot-native'),
      'onScannerState',
      {
        nativeEvent: { state, message: '' },
      },
    );
    await fireEvent.press(screen.getByText('去设置'));
    expect(openSettings).toHaveBeenCalledTimes(1);
  },
);

it('shows the pairing-code alternative without a camera', async () => {
  const onUseCode = jest.fn();
  await render(
    <ScannerSlot active onScan={jest.fn()} onUseCode={onUseCode} />,
  );
  const first = screen.getByTestId('scanner-slot-native');
  await fireEvent(first, 'onScannerState', {
    nativeEvent: { state: 'unavailable', message: '' },
  });
  expect(
    screen.getByText('这台手机没有可用的相机，改用配对码吧'),
  ).toBeOnTheScreen();
  // Saying "use the code" comes with the button that does it.
  await fireEvent.press(screen.getByTestId('scanner-slot-use-code'));
  expect(onUseCode).toHaveBeenCalledTimes(1);
});

it('retries errors by remounting the native view', async () => {
  await render(<ScannerSlot active onScan={jest.fn()} />);
  await fireEvent(screen.getByTestId('scanner-slot-native'), 'onScannerState', {
    nativeEvent: { state: 'error', message: 'failed' },
  });
  const beforeRetry = screen.getByTestId('scanner-slot-native');
  await fireEvent.press(screen.getByText('重试'));
  expect(screen.getByTestId('scanner-slot-native')).not.toBe(beforeRetry);
});

it('keeps the placeholder if the native component is unavailable', async () => {
  jest.mocked(UIManager.hasViewManagerConfig).mockReturnValue(false);
  await render(<ScannerSlot active onScan={jest.fn()} />);
  expect(screen.queryByTestId('scanner-slot-native')).toBeNull();
  expect(screen.getByTestId('scanner-slot-viewfinder')).toBeOnTheScreen();
});

it('renders the native scanner on Android when the view manager is available', async () => {
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: 'android',
  });
  await render(<ScannerSlot active onScan={jest.fn()} />);
  expect(screen.getByTestId('scanner-slot-native')).toBeOnTheScreen();
  expect(screen.queryByTestId('scanner-slot-viewfinder')).toBeOnTheScreen();
});

it('keeps the placeholder on Android when the view manager is unavailable', async () => {
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: 'android',
  });
  jest.mocked(UIManager.hasViewManagerConfig).mockReturnValue(false);
  await render(<ScannerSlot active onScan={jest.fn()} />);
  expect(screen.queryByTestId('scanner-slot-native')).toBeNull();
  expect(screen.getByTestId('scanner-slot-viewfinder')).toBeOnTheScreen();
});
