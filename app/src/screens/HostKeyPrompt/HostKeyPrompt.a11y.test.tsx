import {
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
import HostKeyPromptScreen from './index';

jest.setTimeout(20_000);

const server = sshServers.find(item => item.name === 'gpu-train-02');
if (server === undefined) {
  throw new Error('SSH fixture missing');
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('marks and announces a host-key save error on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  const core = createFakeCore();
  core.failNext('ssh.trustHostKey', new Error('disk full'));
  const platform = createFakePlatform();
  const navigation = mockNavigation<'HostKeyPrompt'>();
  await renderWithProviders(
    <HostKeyPromptScreen
      navigation={navigation}
      route={mockRoute('HostKeyPrompt', {
        sshId: 'ssh1',
        serverId: server.serverId,
        fp: 'SHA256:nT4kP0aZWq8c',
        keyType: 'ED25519',
        kind: 'first',
      })}
    />,
    { core, platform },
  );
  await fireEvent.press(screen.getByText('信任并连接'));
  await waitFor(() =>
    expect(screen.getByText('没能保存指纹：disk full')).toBeOnTheScreen(),
  );
  expect(screen.getByText('没能保存指纹：disk full')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
  expect(announce).toHaveBeenCalledWith('没能保存指纹：disk full');
});
