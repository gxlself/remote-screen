import { act, fireEvent, screen, waitFor } from '@testing-library/react-native';
import { Platform } from 'react-native';
import ToastHost, { hideToast } from '../../components/ToastHost';
import type { AuthCapability } from '../../native/platform/PlatformServices';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { computers } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import AppLockScreen from './index';

afterEach(async () => {
  await act(async () => hideToast());
  jest.restoreAllMocks();
  jest.useRealTimers();
});

async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

async function renderScreen(
  platformResult?: 'ok' | 'cancelled' | 'failed',
  capability?: AuthCapability,
) {
  const navigation = mockNavigation<'AppLock'>();
  const platform = new FakePlatform();
  if (capability !== undefined) {
    platform.authCapabilityResult = capability;
  }
  const rendered = await renderWithProviders(
    <>
      <AppLockScreen navigation={navigation} route={mockRoute('AppLock')} />
      <ToastHost />
    </>,
    {
      platform,
      settings: { 'onboarding.done': true },
      // All five fixture computers paired with this phone.
      localDirectory: {
        computers: computers.map(computer => ({ ...computer, paired: true })),
      },
    },
  );
  if (platformResult !== undefined) {
    rendered.platform.authResult = platformResult;
  }
  await settle();
  return { ...rendered, navigation };
}

function switchChecked(label: string) {
  return screen.getByLabelText(label).props.accessibilityState?.checked;
}

it('starts with launch verification on and session verification off', async () => {
  await renderScreen();
  expect(switchChecked('打开 App 时验证')).toBe(true);
  expect(switchChecked('进入远控前验证')).toBe(false);
});

it('enables session verification without authentication and disables it only after success', async () => {
  const rendered = await renderScreen('ok');
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(true);
  expect(
    rendered.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(0);

  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  expect(rendered.platform.calls).toEqual([
    { method: 'authCapability', args: [] },
    { method: 'authenticate', args: ['关闭 Face ID 保护'] },
  ]);
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(false);
});

it('keeps protection when disabling authentication is cancelled', async () => {
  const rendered = await renderScreen('cancelled');
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(true);
  expect(switchChecked('进入远控前验证')).toBe(true);
  // A cancel is the user's own choice; nothing to explain.
  expect(screen.queryByTestId('global-toast')).toBeNull();
});

it('says why the switch sprang back when verification fails', async () => {
  const rendered = await renderScreen('failed');
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  // S24
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(true);
  expect(screen.getByTestId('global-toast')).toHaveTextContent(
    '没有验证通过，保护没有关',
  );
});

it('reads each switch description as its hint', async () => {
  await renderScreen();
  // S22
  expect(screen.getByLabelText('打开 App 时验证')).toHaveProp(
    'accessibilityHint',
    '从后台回来超过 5 分钟也要验证',
  );
  expect(screen.getByLabelText('进入远控前验证')).toHaveProp(
    'accessibilityHint',
    '每次连接电脑、打开终端前都要验证',
  );
  await waitFor(() =>
    expect(screen.getByLabelText('Studio Mac mini 连接前验证')).toHaveProp(
      'accessibilityHint',
      '连接前总要验证',
    ),
  );
});

it('does not write protection changes while authentication is in flight', async () => {
  const rendered = await renderScreen('ok');
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(true);

  let finishAuth: ((result: 'ok') => void) | undefined;
  const pendingAuth = new Promise<'ok'>(resolve => {
    finishAuth = resolve;
  });
  const authenticate = jest
    .spyOn(rendered.platform, 'authenticate')
    .mockReturnValueOnce(pendingAuth);

  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  expect(authenticate).toHaveBeenCalledTimes(1);
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(true);

  await act(async () => {
    finishAuth?.('ok');
    await pendingAuth;
  });
  expect(rendered.settingsStore.get('lock.beforeSession')).toBe(false);
});

it('lists five computers and marks sensitive hosts after authentication', async () => {
  const rendered = await renderScreen('ok');
  await waitFor(() => {
    expect(screen.getByText('Studio Mac mini')).toBeOnTheScreen();
  });
  expect(screen.getByText('工位 Windows')).toBeOnTheScreen();
  expect(screen.getByText('小明的 MacBook Pro')).toBeOnTheScreen();
  expect(screen.getByText('家里台式机')).toBeOnTheScreen();
  expect(screen.getByText('实验室 Mac')).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('Studio Mac mini 连接前验证'));
  await settle();
  expect(rendered.settingsStore.get('lock.sensitiveHosts')).toEqual([
    'h7v2m9q3k8x4tb1a',
  ]);

  rendered.platform.authResult = 'cancelled';
  await fireEvent.press(screen.getByLabelText('Studio Mac mini 连接前验证'));
  await settle();
  expect(rendered.settingsStore.get('lock.sensitiveHosts')).toEqual([
    'h7v2m9q3k8x4tb1a',
  ]);
  expect(
    rendered.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(1);
});

it('shows the fixed verification row and returns to settings', async () => {
  const rendered = await renderScreen();
  // S15: the key row matches Settings: the private key never leaves the chip.
  expect(screen.queryByText('查看或导出 SSH 私钥')).toBeNull();
  expect(screen.getByTestId('app-lock-key-fact')).toHaveTextContent(
    'SSH 私钥：存在安全芯片里，导不出来',
  );
  expect(screen.queryByRole('button', { name: /SSH 私钥/ })).toBeNull();

  await fireEvent.press(screen.getByLabelText('返回设置'));
  expect(rendered.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('dims the back button while pressed on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  await renderScreen();
  const back = screen.getByTestId('app-lock-back');
  // S23
  expect(back).not.toHaveStyle({ opacity: 0.6 });
  await fireEvent(back, 'responderGrant', {
    persist: () => {},
    nativeEvent: {},
    currentTarget: 1,
  });
  expect(back).toHaveStyle({ opacity: 0.6 });
});

it('uses Touch ID in the title, description, and authentication reason', async () => {
  const rendered = await renderScreen('ok', {
    biometry: 'touchId',
    passcode: true,
  });

  expect(screen.getByText('Touch ID 解锁')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '用 Touch ID 保护这台手机上的远控入口。验证在手机上完成，不会上传。',
    ),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  await fireEvent.press(screen.getByLabelText('进入远控前验证'));
  await settle();
  expect(rendered.platform.calls).toContainEqual({
    method: 'authenticate',
    args: ['关闭 Touch ID 保护'],
  });
});
