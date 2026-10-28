import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { Platform } from 'react-native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import { fallback } from '../../i18n/zh/fallback';
import { FallbackGuide } from './FallbackGuide';

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('renders nothing while there is no guide', async () => {
  await renderWithProviders(<FallbackGuide guide={null} onClose={jest.fn()} />);
  expect(screen.queryByText('需要 Windows App')).toBeNull();
});

it('opens the RDP store page and closes the guide', async () => {
  const platform = new FakePlatform();
  const onClose = jest.fn();
  await renderWithProviders(
    <FallbackGuide guide={{ kind: 'rdp' }} onClose={onClose} />,
    { platform },
  );

  expect(screen.getByText('需要 Windows App')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '去 App Store' }));

  expect(platform.opened).toEqual([
    'itms-apps://apps.apple.com/app/id714464092',
  ]);
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('shows the Mac VNC explanation and can be cancelled', async () => {
  const onClose = jest.fn();
  await renderWithProviders(
    <FallbackGuide guide={{ kind: 'vnc' }} onClose={onClose} />,
  );

  expect(screen.getByText('需要一个屏幕共享 App')).toBeOnTheScreen();
  expect(
    screen.getByText(
      'iPhone 没有自带屏幕共享。装一个屏幕共享 App（比如 VNC Viewer）后再点一次；电脑上要先打开「系统设置 › 通用 › 共享 › 屏幕共享」。',
    ),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByRole('button', { name: '取消' }));
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('opens the Android RDP listing through Google Play', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const platform = new FakePlatform();
  const onClose = jest.fn();
  await renderWithProviders(
    <FallbackGuide guide={{ kind: 'rdp' }} onClose={onClose} />,
    { platform },
  );

  expect(screen.getByText('需要 Windows App')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '去 Google Play' }));

  await waitFor(() =>
    expect(platform.opened).toEqual([
      'market://details?id=com.microsoft.rdc.androidx',
    ]),
  );
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('tries the web Play Store page when the Android market link fails', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const onClose = jest.fn();
  await renderWithProviders(
    <FallbackGuide guide={{ kind: 'rdp' }} onClose={onClose} />,
    { platform },
  );

  await fireEvent.press(screen.getByRole('button', { name: '去 Google Play' }));

  await waitFor(() =>
    expect(platform.opened).toEqual([
      'market://details?id=com.microsoft.rdc.androidx',
      'https://play.google.com/store/apps/details?id=com.microsoft.rdc.androidx',
    ]),
  );
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('shows the Android VNC explanation', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderWithProviders(
    <FallbackGuide guide={{ kind: 'vnc' }} onClose={jest.fn()} />,
  );

  expect(screen.getByText('需要一个屏幕共享 App')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '手机没有自带屏幕共享。装一个屏幕共享 App（比如 VNC Viewer）后再点一次；电脑上要先打开「系统设置 › 通用 › 共享 › 屏幕共享」。',
    ),
  ).toBeOnTheScreen();
  expect(
    screen.queryByText(
      'iPhone 没有自带屏幕共享。装一个屏幕共享 App（比如 VNC Viewer）后再点一次；电脑上要先打开「系统设置 › 通用 › 共享 › 屏幕共享」。',
    ),
  ).toBeNull();
});

it('says iPad instead of iPhone on an iPad (V56)', async () => {
  const platform = new FakePlatform();
  platform.info = { ...platform.info, isPad: true };
  await renderWithProviders(
    <FallbackGuide guide={{ kind: 'vnc' }} onClose={jest.fn()} />,
    { platform },
  );

  expect(screen.getByText(fallback.guide.vnc.messagePad)).toBeOnTheScreen();
  expect(screen.queryByText(fallback.guide.vnc.message)).toBeNull();
});
