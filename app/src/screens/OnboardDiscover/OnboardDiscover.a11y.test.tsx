import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { AccessibilityInfo, PixelRatio } from 'react-native';
import { focusManager } from '@tanstack/react-query';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import OnboardDiscoverScreen from './index';

async function renderScreen(
  platform = new FakePlatform(),
  core = createFakeCore(),
) {
  const rendered = await renderWithProviders(
    <OnboardDiscoverScreen
      navigation={mockNavigation<'OnboardDiscover'>()}
      route={mockRoute('OnboardDiscover')}
    />,
    { core, platform, localDirectory: {} },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...rendered, platform, core };
}

beforeEach(() => {
  jest.useFakeTimers();
  focusManager.setFocused(true);
});

afterEach(async () => {
  await cleanup();
  focusManager.setFocused(undefined);
  jest.useRealTimers();
  jest.restoreAllMocks();
});

it('announces the directory result once', async () => {
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockClear();
  await renderScreen();
  await screen.findByText('找到了 4 台电脑');
  await waitFor(() => expect(announce).toHaveBeenCalledWith('找到了 4 台电脑'));
  expect(
    announce.mock.calls.filter(call => call[0] === '找到了 4 台电脑'),
  ).toHaveLength(1);
});

it('does not announce a directory already present on first render', async () => {
  const first = await renderScreen();
  expect(await screen.findByText('找到了 4 台电脑')).toBeOnTheScreen();
  await first.unmount();
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockClear();

  // Coming back to the screen: the directory is already loaded.
  await render(
    <OnboardDiscoverScreen
      navigation={mockNavigation<'OnboardDiscover'>()}
      route={mockRoute('OnboardDiscover')}
    />,
    { wrapper: first.wrapper },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });

  expect(screen.getByText('找到了 4 台电脑')).toBeOnTheScreen();
  expect(announce).not.toHaveBeenCalled();
});

it('stacks the footer and moves it into the scroll content for large text', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const rendered = await renderScreen();
  const scroll = screen.getByTestId('onboard-discover-scroll');
  const footer = screen.getByTestId('onboard-discover-footer');
  expect(footer).toHaveStyle({
    flexDirection: 'column',
    alignItems: 'stretch',
  });
  expect(
    within(footer)
      .getAllByRole('button')
      .map(button => button.props.accessibilityLabel),
  ).toEqual(['继续']);
  expect(within(scroll).getByText('继续')).toBeOnTheScreen();
  await rendered.unmount();

  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(1);
  await renderScreen();
  const regularScroll = screen.getByTestId('onboard-discover-scroll');
  expect(within(regularScroll).queryByText('继续')).toBeNull();
  expect(screen.getByText('继续')).toBeOnTheScreen();
  expect(screen.queryByText('跳过')).toBeNull();
});
