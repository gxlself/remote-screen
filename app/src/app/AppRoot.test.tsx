import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  within,
} from '@testing-library/react-native';
import { Dimensions, useWindowDimensions } from 'react-native';
import { createMMKV } from 'react-native-mmkv';
import App from '../../App';
import { hideToast, showToast } from '../components/ToastHost';
import SelfTest from '../screens/dev/SelfTest';
import { mockPhoneDimensions, mockWindowDimensions } from '../test/dimensions';
import AppRoot from './AppRoot';

jest.mock('../screens/dev/SelfTest', () => {
  const component = jest.fn(() => null);
  Object.defineProperty(component, 'name', { value: 'SelfTest' });
  return component;
});

it('opens onboarding on first launch inside the app providers', async () => {
  await render(<AppRoot />);
  // Tailscale is opt-in: onboarding starts at finding computers, step 1 of 2.
  expect(screen.getByTestId('onboard-discover-scroll')).toBeOnTheScreen();
  expect(screen.getByText('1 / 2')).toBeOnTheScreen();
  expect(screen.queryByText('连接到你的 tailnet')).toBeNull();
});

it('mounts the global toast over every screen', async () => {
  await render(<AppRoot />);
  await act(async () => showToast({ message: '已保存', tone: 'good' }));
  expect(screen.getByTestId('global-toast')).toHaveProp(
    'accessibilityLabel',
    '已保存',
  );
  await act(async () => hideToast());
  expect(screen.queryByTestId('global-toast')).toBeNull();
});

it('opens Home after onboarding is complete', async () => {
  const createMMKVSpy = jest.mocked(createMMKV);
  const createMMKVImpl = createMMKVSpy.getMockImplementation();
  const storage = createMMKV();
  storage.set('settings.onboarding.done', 'true');
  createMMKVSpy.mockReturnValue(storage);
  const restoreDimensions = mockPhoneDimensions();
  try {
    await render(<AppRoot />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByTestId('home-scroll')).toBeOnTheScreen();
  } finally {
    restoreDimensions();
    if (createMMKVImpl === undefined) {
      createMMKVSpy.mockReset();
    } else {
      createMMKVSpy.mockImplementation(createMMKVImpl);
    }
  }
});

it('renders the wide two-column home at 1024 px', async () => {
  const createMMKVSpy = jest.mocked(createMMKV);
  const createMMKVImpl = createMMKVSpy.getMockImplementation();
  const storage = createMMKV();
  storage.set('settings.onboarding.done', 'true');
  createMMKVSpy.mockReturnValue(storage);
  const restoreDimensions = mockWindowDimensions({
    width: 1024,
    height: 768,
  });
  try {
    const view = await render(<AppRoot />);
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByTestId('home-wide')).toBeOnTheScreen();
    const sidebar = within(screen.getByTestId('wide-sidebar'));
    expect(sidebar.getByText('设备')).toBeOnTheScreen();
    expect(
      (await sidebar.findAllByTestId(/^wide-computer-/)).length,
    ).toBeGreaterThan(0);

    await fireEvent.press(sidebar.getByLabelText('添加设备'));
    // V10G: Add opens Add Computer first; installing Host is the other way.
    expect(screen.getByRole('header', { name: '添加电脑' })).toBeOnTheScreen();
    await view.unmount();
  } finally {
    restoreDimensions();
    if (createMMKVImpl === undefined) {
      createMMKVSpy.mockReset();
    } else {
      createMMKVSpy.mockImplementation(createMMKVImpl);
    }
  }
});

it('keeps the unattended launch entry in the real app', async () => {
  await render(<App autoConnect="100.64.0.7:7400" />);
  expect(
    await screen.findByTestId('onboard-discover-scroll'),
  ).toBeOnTheScreen();
  expect(SelfTest).not.toHaveBeenCalled();
});

it('renders the M0 self-test through the dev harness', async () => {
  await render(<App devHarness="selftest" harnessArg="100.64.0.7:7400" />);
  expect(SelfTest).toHaveBeenLastCalledWith(
    { autoConnect: '100.64.0.7:7400' },
    undefined,
  );
});

it('overrides window dimensions and restores the actual hook', async () => {
  const original = Dimensions.get('window');
  const restore = mockWindowDimensions({ width: 1024, height: 768 });
  try {
    const wide = await renderHook(() => useWindowDimensions());
    expect(wide.result.current).toEqual({
      ...original,
      width: 1024,
      height: 768,
    });
    expect(Dimensions.get('window')).toEqual(original);
    await wide.unmount();
  } finally {
    restore();
  }
  const actual = await renderHook(() => useWindowDimensions());
  expect(actual.result.current).toEqual(original);
});
