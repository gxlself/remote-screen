import { act, fireEvent, screen } from '@testing-library/react-native';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { resetSshStore, sshStore } from '../../features/sessions/sshStore';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import AppLockGate from './index';

let listeners: ((state: AppStateStatus) => void)[] = [];

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  listeners = [];
  jest
    .spyOn(AppState, 'addEventListener')
    .mockImplementation((_event, listener) => {
      listeners.push(listener);
      return {
        remove: () => {
          listeners = listeners.filter(entry => entry !== listener);
        },
      };
    });
});

afterEach(() => {
  resetSshStore();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

async function changeAppState(state: AppStateStatus) {
  await act(async () => {
    for (const listener of listeners) {
      listener(state);
    }
  });
  await settle();
}

function lockedPlatform() {
  const platform = new FakePlatform();
  platform.authResult = 'cancelled';
  return platform;
}

it('locks on launch, auto-authenticates once, and unlocks with Face ID', async () => {
  const platform = lockedPlatform();
  const rendered = await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });

  await settle();
  expect(screen.getByText('Remote Screen 已锁定')).toBeOnTheScreen();
  expect(rendered.platform.calls).toEqual([
    { method: 'authCapability', args: [] },
    { method: 'authenticate', args: ['解锁 Remote Screen'] },
  ]);

  rendered.platform.authResult = 'ok';
  await fireEvent.press(screen.getByText('用 Face ID 解锁'));
  await settle();
  expect(screen.queryByText('Remote Screen 已锁定')).toBeNull();
});

it('does not lock during onboarding or when opening verification is off', async () => {
  const onboarding = await renderWithProviders(<AppLockGate />, {
    settings: { 'onboarding.done': false },
  });
  expect(screen.queryByText('Remote Screen 已锁定')).toBeNull();
  expect(
    onboarding.platform.calls.some(call => call.method === 'authenticate'),
  ).toBe(false);
  await onboarding.unmount();

  await renderWithProviders(<AppLockGate />, {
    settings: { 'onboarding.done': true, 'lock.onOpen': false },
  });
  expect(screen.queryByText('Remote Screen 已锁定')).toBeNull();
});

it('locks only after five minutes in the background', async () => {
  const platform = lockedPlatform();
  const rendered = await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  rendered.platform.authResult = 'ok';
  await settle();
  await fireEvent.press(screen.getByText('用 Face ID 解锁'));
  await settle();
  expect(screen.queryByText('Remote Screen 已锁定')).toBeNull();

  await changeAppState('background');
  await act(async () => {
    jest.advanceTimersByTime(4 * 60 * 1000);
  });
  await changeAppState('active');
  expect(screen.queryByText('Remote Screen 已锁定')).toBeNull();

  await changeAppState('background');
  await act(async () => {
    jest.advanceTimersByTime(5 * 60 * 1000);
  });
  rendered.platform.authResult = 'cancelled';
  await changeAppState('active');
  expect(screen.getByText('Remote Screen 已锁定')).toBeOnTheScreen();
  expect(
    rendered.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(3);
});

it('keeps the lock on failure and unlocks when authentication is unavailable', async () => {
  const platform = lockedPlatform();
  const rendered = await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  rendered.platform.authResult = 'failed';

  await fireEvent.press(screen.getByText('用 Face ID 解锁'));
  await settle();
  expect(screen.getByText('没有验证通过，再试一次')).toBeOnTheScreen();
  expect(screen.getByText('Remote Screen 已锁定')).toBeOnTheScreen();

  rendered.platform.authResult = 'unavailable';
  await fireEvent.press(screen.getByText('用 Face ID 解锁'));
  await settle();
  expect(screen.queryByText('Remote Screen 已锁定')).toBeNull();
  expect(
    rendered.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(3);
});

it('shows the live session and refreshes its clock every second', async () => {
  const platform = lockedPlatform();
  await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  expect(screen.getByText('有 1 个远控会话在后台保持')).toBeOnTheScreen();
  expect(screen.getByText('Studio Mac mini · 42:18')).toBeOnTheScreen();

  await act(async () => {
    jest.advanceTimersByTime(1000);
  });
  expect(screen.getByText('Studio Mac mini · 42:19')).toBeOnTheScreen();
});

it('points to the passcode in the system prompt instead of a second button', async () => {
  const platform = lockedPlatform();
  const rendered = await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  await settle();

  // S13: a 「用设备密码」 button raised the same Face ID prompt again.
  expect(screen.queryByText('用设备密码')).toBeNull();
  expect(screen.queryByRole('button', { name: /设备密码/ })).toBeNull();
  expect(screen.getByText('验证弹窗里也可以改用设备密码')).toBeOnTheScreen();
  expect(
    rendered.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(1);
});

it('counts terminals kept in the background with remote sessions', async () => {
  sshStore.apply(
    { type: 'ssh.state', sshId: 'ssh1', serverId: 'a', state: 'open' },
    FIXTURE_NOW,
  );
  sshStore.apply(
    { type: 'ssh.state', sshId: 'ssh2', serverId: 'b', state: 'open' },
    FIXTURE_NOW,
  );
  await renderWithProviders(<AppLockGate />, {
    platform: lockedPlatform(),
    settings: { 'onboarding.done': true },
  });
  // S25: real counts, SSH included.
  expect(
    screen.getByText('有 1 个远控会话、2 个终端在后台保持'),
  ).toBeOnTheScreen();
  expect(screen.getByText('Studio Mac mini · 42:18')).toBeOnTheScreen();
});

it('shows a terminal-only card without a session clock', async () => {
  sshStore.apply(
    { type: 'ssh.state', sshId: 'ssh1', serverId: 'a', state: 'open' },
    FIXTURE_NOW,
  );
  await renderWithProviders(<AppLockGate />, {
    core: createFakeCore({ sessions: [] }),
    platform: lockedPlatform(),
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(screen.getByText('有 1 个终端在后台保持')).toBeOnTheScreen();
  expect(screen.queryByText(/Studio Mac mini/)).toBeNull();
});

it('shows Touch ID when the device supports it', async () => {
  const platform = lockedPlatform();
  platform.authCapabilityResult = { biometry: 'touchId', passcode: true };

  await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  await settle();

  expect(screen.getByText('用 Touch ID 解锁')).toBeOnTheScreen();
  expect(screen.getByText('验证弹窗里也可以改用设备密码')).toBeOnTheScreen();
});

it('uses the device passcode as the primary action without a passcode fallback button', async () => {
  const platform = lockedPlatform();
  platform.authCapabilityResult = { biometry: 'none', passcode: true };

  await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  await settle();

  expect(screen.getByText('用设备密码解锁')).toBeOnTheScreen();
  expect(screen.queryByText('用设备密码')).toBeNull();
  expect(screen.queryByText('验证弹窗里也可以改用设备密码')).toBeNull();
});

it('refreshes the authentication name when the app returns to the foreground', async () => {
  const platform = lockedPlatform();
  await renderWithProviders(<AppLockGate />, {
    platform,
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(screen.getByText('用 Face ID 解锁')).toBeOnTheScreen();

  platform.authCapabilityResult = { biometry: 'touchId', passcode: true };
  await changeAppState('background');
  await changeAppState('active');

  expect(screen.getByText('用 Touch ID 解锁')).toBeOnTheScreen();
});
