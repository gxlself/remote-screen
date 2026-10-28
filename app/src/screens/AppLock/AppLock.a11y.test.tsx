import { act, screen } from '@testing-library/react-native';
import { Platform } from 'react-native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit, space } from '../../theme/tokens';
import AppLockScreen from '.';

async function renderScreen() {
  const rendered = await renderWithProviders(
    <AppLockScreen
      navigation={mockNavigation<'AppLock'>()}
      route={mockRoute('AppLock')}
    />,
    { platform: new FakePlatform(), settings: { 'onboarding.done': true } },
  );
  await act(() => Promise.resolve());
  return rendered;
}

afterEach(() => {
  jest.restoreAllMocks();
});

it.each(['ios', 'android'] as const)(
  'gives the %s back button the platform hit target and safe-area padding',
  async platform => {
    jest.replaceProperty(Platform, 'OS', platform);
    await renderScreen();
    expect(screen.getByLabelText('返回设置')).toHaveStyle({
      minHeight: hit.min,
    });
    const scroll = screen.getByTestId('app-lock-scroll');
    expect(scroll?.props.contentContainerStyle).toEqual(
      expect.arrayContaining([{ paddingTop: 47 + space.sm }]),
    );
  },
);
