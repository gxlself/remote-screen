import { cleanup, screen, within } from '@testing-library/react-native';
import { PixelRatio, Platform } from 'react-native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import OnboardProtectScreen from './index';

async function renderScreen(platform = new FakePlatform()) {
  return renderWithProviders(
    <OnboardProtectScreen
      navigation={mockNavigation<'OnboardProtect'>()}
      route={mockRoute('OnboardProtect')}
    />,
    { platform },
  );
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('omits the iOS local-network permission card on Android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const platform = new FakePlatform();
  await renderScreen(platform);
  await screen.findByText('保护这个 App');

  expect(screen.queryByText('本地网络权限')).toBeNull();
  expect(screen.queryByText('点「允许」后，iOS 会再问你一次。')).toBeNull();
  expect(
    platform.calls.some(call => call.method === 'requestLocalNetwork'),
  ).toBe(false);
});

it('moves the start action into the scroll content for large text', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const rendered = await renderScreen();
  await screen.findByText('开始使用');
  expect(
    within(screen.getByTestId('onboard-protect-scroll')).getByText('开始使用'),
  ).toBeOnTheScreen();
  await rendered.unmount();

  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(1);
  await renderScreen();
  await screen.findByText('开始使用');
  expect(
    within(screen.getByTestId('onboard-protect-scroll')).queryByText(
      '开始使用',
    ),
  ).toBeNull();
  expect(screen.getByText('开始使用')).toBeOnTheScreen();
});
