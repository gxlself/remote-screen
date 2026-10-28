import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import {
  AccessibilityInfo,
  AppState,
  BackHandler,
  Platform,
} from 'react-native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import AppLockGate from '.';

async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

async function renderGate(platform: 'ios' | 'android') {
  jest.replaceProperty(Platform, 'OS', platform);
  const fake = new FakePlatform();
  fake.authResult = 'cancelled';
  const rendered = await renderWithProviders(<AppLockGate />, {
    platform: fake,
    settings: { 'onboarding.done': true },
  });
  await settle();
  return { ...rendered, fake };
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  jest.clearAllMocks();
  jest.spyOn(AppState, 'addEventListener').mockReturnValue({
    remove: jest.fn(),
  });
});

afterEach(() => {
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it.each(['ios', 'android'] as const)(
  'uses BackHandler.exitApp for %s only on Android',
  async platform => {
    const exit = jest.spyOn(BackHandler, 'exitApp');
    await renderGate(platform);

    await fireEvent(screen.getByTestId('app-lock-gate'), 'requestClose');
    expect(exit).toHaveBeenCalledTimes(platform === 'android' ? 1 : 0);
  },
);

it('shows the failure live region on Android and announces it on iOS', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const ios = await renderGate('ios');
  ios.fake.authResult = 'failed';
  await fireEvent.press(screen.getByText('用 Face ID 解锁'));
  await settle();
  expect(screen.getByText('没有验证通过，再试一次')).toBeOnTheScreen();
  expect(announce).toHaveBeenCalledWith('没有验证通过，再试一次');

  await cleanup();
  announce.mockClear();
  const android = await renderGate('android');
  android.fake.authResult = 'failed';
  await fireEvent.press(screen.getByText('用 Face ID 解锁'));
  await settle();
  expect(screen.getByText('没有验证通过，再试一次')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
  expect(announce).not.toHaveBeenCalled();
});

it('makes the locked content scrollable with the session visible', async () => {
  await renderGate('ios');
  expect(screen.getByTestId('app-lock-gate-scroll')).toHaveProp(
    'contentContainerStyle',
    { flexGrow: 1, justifyContent: 'center' },
  );
  expect(screen.getByText('Studio Mac mini · 42:18')).toBeOnTheScreen();
});
