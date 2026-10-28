import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { AccessibilityInfo, PixelRatio, Platform } from 'react-native';
import { tailscaleUrls } from '../../features/platform/externalApps';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { netNoTailnet, netTailnetOk } from '../../native/rscore/fixtures';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import OnboardTailnetScreen from './index';

async function renderScreen(platform = new FakePlatform()) {
  const core = createFakeCore();
  core.failNext('network.status', new Error('network unavailable'));
  const rendered = await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, platform, settings: { 'tailnet.enabled': true } },
  );
  return { ...rendered, core, platform };
}

function keyboardBehavior(instance: { unstable_fiber?: unknown }) {
  let fiber = instance.unstable_fiber as
    | { memoizedProps?: Record<string, unknown>; return?: unknown }
    | null
    | undefined;
  while (fiber != null) {
    if (fiber.memoizedProps?.behavior !== undefined) {
      return fiber.memoizedProps.behavior;
    }
    fiber = fiber.return as typeof fiber;
  }
  return undefined;
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('exposes the connection state and details on the system radio card', async () => {
  const { core } = await renderScreen();
  await act(() => core.emit({ type: 'network.changed', status: netTailnetOk }));

  const system = screen.getByLabelText('使用手机上的 Tailscale');
  expect(system.props.accessibilityState).toMatchObject({
    checked: true,
    selected: true,
  });
  expect(system.props.accessibilityHint).toBe(
    '检测到 Tailscale 正在运行。本机地址 100.64.0.21',
  );
  expect(screen.queryByLabelText('在 App 内连接')).toBeNull();
});

it('announces each tailnet status change but not the first render', async () => {
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockClear();
  const { core } = await renderScreen();
  expect(announce).not.toHaveBeenCalled();

  await act(() => core.emit({ type: 'network.changed', status: netTailnetOk }));
  expect(announce).toHaveBeenCalledWith('已连接');

  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));
  expect(announce.mock.calls.map(call => call[0])).toEqual([
    '已连接',
    '未连接',
  ]);
});

it('opens the Android Tailscale URL and shows the Android ripple', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const platform = new FakePlatform();
  const { core } = await renderScreen(platform);
  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));

  const card = screen.getByLabelText('使用手机上的 Tailscale');
  expect(card.props.nativeBackgroundAndroid).toMatchObject({
    type: 'RippleAndroid',
    borderless: false,
  });

  await fireEvent.press(screen.getByText('打开 Tailscale'));
  await waitFor(() => expect(platform.opened).toHaveLength(1));
  expect(platform.opened[0]).toBe(tailscaleUrls()[0]);
});

it('keeps the iOS URL and omits the Android ripple', async () => {
  const platform = new FakePlatform();
  const { core } = await renderScreen(platform);
  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));

  expect(
    screen.getByLabelText('使用手机上的 Tailscale').props
      .nativeBackgroundAndroid,
  ).toBeUndefined();
  await fireEvent.press(screen.getByText('打开 Tailscale'));
  await waitFor(() => expect(platform.opened).toHaveLength(1));
  expect(platform.opened[0]).toBe('tailscale://');
});

it('falls back to the iOS store URL when Tailscale cannot be opened', async () => {
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const { core } = await renderScreen(platform);
  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));

  await fireEvent.press(screen.getByText('打开 Tailscale'));
  await waitFor(() => expect(platform.opened).toHaveLength(2));
  expect(platform.opened[1]).toBe(tailscaleUrls()[1]);
});

it('moves the primary action into the scroll content for large text', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const rendered = await renderScreen();
  expect(
    within(screen.getByTestId('onboard-tailnet-scroll')).getByText('继续'),
  ).toBeOnTheScreen();
  await rendered.unmount();

  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(1);
  await renderScreen();
  expect(
    within(screen.getByTestId('onboard-tailnet-scroll')).queryByText('继续'),
  ).toBeNull();
  expect(screen.getByText('继续')).toBeOnTheScreen();
});

it('keeps the page and footer still on iOS and insets only the form scroll', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(1);
  await renderScreen();
  expect(
    keyboardBehavior(screen.getByTestId('onboard-tailnet')),
  ).toBeUndefined();
  const scroll = screen.getByTestId('onboard-tailnet-scroll');
  expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
  expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
  expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
  expect(scroll).toContainElement(
    screen.getByLabelText('使用手机上的 Tailscale'),
  );
  expect(scroll).not.toContainElement(screen.getByText('继续'));
});

it('uses padding keyboard avoidance on android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderScreen();
  expect(keyboardBehavior(screen.getByTestId('onboard-tailnet'))).toBe(
    'padding',
  );
});
