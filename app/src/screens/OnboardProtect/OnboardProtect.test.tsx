import { fireEvent, screen } from '@testing-library/react-native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { MemoryKv } from '../../features/storage/kv';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import OnboardProtectScreen from './index';

async function renderScreen(
  platform = new FakePlatform(),
  kv = new MemoryKv(),
) {
  const navigation = mockNavigation<'OnboardProtect'>();
  const rendered = await renderWithProviders(
    <OnboardProtectScreen
      navigation={navigation}
      route={mockRoute('OnboardProtect')}
    />,
    { platform, kv },
  );
  return { navigation, platform, kv, rendered };
}

it('keeps Face ID enabled by default and writes the lock key', async () => {
  const { kv, platform } = await renderScreen();
  await screen.findByText('Face ID 解锁');
  const faceId = screen.getByLabelText('Face ID 解锁');
  expect(faceId.props.accessibilityState.checked).toBe(true);

  await fireEvent.press(faceId);
  expect(kv.getJSON('settings.lock.onOpen')).toBe(false);
  expect(screen.getByLabelText('Face ID 解锁')).not.toBeChecked();

  platform.authResult = 'cancelled';
  await fireEvent.press(screen.getByLabelText('Face ID 解锁'));
  expect(kv.getJSON('settings.lock.onOpen')).toBe(false);
  expect(platform.calls.some(call => call.method === 'authenticate')).toBe(
    true,
  );

  platform.authResult = 'ok';
  await fireEvent.press(screen.getByLabelText('Face ID 解锁'));
  expect(kv.getJSON('settings.lock.onOpen')).toBe(true);
});

it('explains a missing Face ID or passcode', async () => {
  const { platform } = await renderScreen();
  await screen.findByText('Face ID 解锁');
  await fireEvent.press(screen.getByLabelText('Face ID 解锁'));
  platform.authResult = 'unavailable';
  await fireEvent.press(screen.getByLabelText('Face ID 解锁'));
  expect(
    await screen.findByText('这台设备没有设置 Face ID 或密码'),
  ).toBeOnTheScreen();
  expect(screen.getByLabelText('Face ID 解锁')).not.toBeChecked();
});

it('reports unavailable without authenticating when no method is configured', async () => {
  const platform = new FakePlatform();
  platform.authCapabilityResult = { biometry: 'none', passcode: false };
  const kv = new MemoryKv();
  kv.setJSON('settings.lock.onOpen', false);
  await renderScreen(platform, kv);

  await screen.findByText('Face ID 解锁');
  await fireEvent.press(screen.getByLabelText('Face ID 解锁'));

  expect(
    await screen.findByText('这台设备没有设置 Face ID 或密码'),
  ).toBeOnTheScreen();
  expect(platform.calls.some(call => call.method === 'authenticate')).toBe(
    false,
  );
  expect(kv.getJSON('settings.lock.onOpen')).toBe(false);
});

it('enables notifications only after permission is granted', async () => {
  const { kv } = await renderScreen();
  await screen.findByText('通知');
  expect(screen.getByLabelText('通知')).not.toBeChecked();

  await fireEvent.press(screen.getByLabelText('通知'));
  expect(kv.getJSON('settings.notify.enabled')).toBe(true);

  await fireEvent.press(screen.getByLabelText('通知'));
  expect(kv.getJSON('settings.notify.enabled')).toBe(false);

  const denied = new FakePlatform();
  denied.requestNotifications = jest.fn(async () => 'denied' as const);
  const deniedKv = new MemoryKv();
  const deniedScreen = await renderScreen(denied, deniedKv);
  await screen.findByText('通知');
  await fireEvent.press(screen.getByLabelText('通知'));
  expect(screen.getByLabelText('通知')).not.toBeChecked();
  await deniedScreen.rendered.unmount();
});

it('requests local network access and shows the allowed chip', async () => {
  const { platform } = await renderScreen();
  await screen.findByText('本地网络权限');
  expect(
    screen.getByText('点「允许」后，iOS 会再问你一次。'),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('允许'));
  expect(
    platform.calls.some(call => call.method === 'requestLocalNetwork'),
  ).toBe(true);
  expect(await screen.findByText('已允许')).toBeOnTheScreen();
  expect(
    screen.getByText('已允许，会自动发现同一网络里的电脑'),
  ).toBeOnTheScreen();
});

it('counts the Tailscale step and names it only while it is switched on', async () => {
  await renderScreen(
    new FakePlatform(),
    new MemoryKv({ 'settings.tailnet.enabled': 'true' }),
  );
  await screen.findByText('开始使用');
  expect(screen.getByText('3 / 3')).toBeOnTheScreen();
  expect(screen.getByLabelText('第 3 步，共 3 步')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '用来发现同一 Wi-Fi 里的电脑，局域网直连更快。不开也能用，只是都走 Tailscale。',
    ),
  ).toBeOnTheScreen();
});

it('finishes onboarding and resets navigation to the main tabs', async () => {
  const { kv, navigation } = await renderScreen();
  await screen.findByText('开始使用');
  // Tailscale is off by default, so there is no Tailscale step before this.
  expect(screen.getByText('2 / 2')).toBeOnTheScreen();
  expect(screen.getByLabelText('第 2 步，共 2 步')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '用来发现同一 Wi-Fi 里的电脑，局域网直连更快。不开的话，只能按地址手动添加电脑。',
    ),
  ).toBeOnTheScreen();
  expect(screen.queryByText(/Tailscale/)).toBeNull();
  expect(screen.getByText('之后可以在 设置 › 安全 中更改')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('开始使用'));
  expect(kv.getJSON('settings.onboarding.done')).toBe(true);
  expect(navigation.reset).toHaveBeenCalledWith({
    index: 0,
    routes: [{ name: 'MainTabs' }],
  });

  await fireEvent.press(screen.getByText('上一步'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});
