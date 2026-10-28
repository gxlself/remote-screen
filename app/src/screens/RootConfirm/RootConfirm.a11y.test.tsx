import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { AccessibilityInfo, Platform } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { sshServers } from '../../native/rscore/fixtures';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import RootConfirmScreen from './index';

jest.mock('../../components/StepList', () => ({
  StepList: (stepProps: { reduceMotion?: boolean }) => {
    const React = require('react') as typeof import('react');
    const { View } = require('react-native') as typeof import('react-native');
    return React.createElement(View, {
      testID: 'step-list-probe',
      ...stepProps,
    });
  },
}));

jest.setTimeout(20_000);

const server = sshServers.find(item => item.name === 'prod-api-01');
if (server === undefined) {
  throw new Error('SSH fixture missing');
}

function props() {
  return {
    navigation: mockNavigation<'RootConfirm'>(),
    route: mockRoute('RootConfirm', {
      sshId: 'ssh1',
      serverId: server!.serverId,
      url: 'https://hs.example.com/a/xyz',
      user: 'root',
    }),
  };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('passes the system reduce-motion setting to the step list', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isReduceMotionEnabled')
    .mockResolvedValue(true);
  const view = await renderWithProviders(<RootConfirmScreen {...props()} />);
  await act(async () => {
    await Promise.resolve();
  });
  await waitFor(() => {
    expect(view.getByTestId('step-list-probe')).toHaveProp(
      'reduceMotion',
      true,
    );
  });
});

it('announces a browser-open failure on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  const platform = createFakePlatform();
  platform.openUrlResult = false;
  await renderWithProviders(<RootConfirmScreen {...props()} />, {
    core: createFakeCore(),
    platform,
  });
  await fireEvent.press(screen.getByText('用 Face ID 确认，并在浏览器登录'));
  await waitFor(() =>
    expect(
      screen.getByText('打不开浏览器，请复制链接手动打开'),
    ).toBeOnTheScreen(),
  );
  expect(announce).toHaveBeenCalledWith('打不开浏览器，请复制链接手动打开');
  expect(screen.getByText('打不开浏览器，请复制链接手动打开')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
});

it.each(['ios', 'android'] as const)('announces copied on %s', async os => {
  jest.replaceProperty(Platform, 'OS', os);
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  const platform = createFakePlatform();
  platform.openUrlResult = false;
  await renderWithProviders(<RootConfirmScreen {...props()} />, { platform });
  await fireEvent.press(screen.getByText('用 Face ID 确认，并在浏览器登录'));
  await waitFor(() => screen.getByText('复制链接'));
  announce.mockClear();
  await fireEvent.press(screen.getByText('复制链接'));
  expect(announce).toHaveBeenCalledWith('已复制');
});
