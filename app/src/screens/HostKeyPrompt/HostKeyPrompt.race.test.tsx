import { act, cleanup, screen, waitFor } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { sshServers } from '../../native/rscore/fixtures';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import HostKeyPromptScreen from './index';

const server = sshServers.find(item => item.name === 'gpu-train-02');
if (server === undefined) {
  throw new Error('gpu-train-02 fixture missing');
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('keeps a changed-key prompt open when the expected failed state follows it', async () => {
  const core = createFakeCore();
  const navigation = mockNavigation<'HostKeyPrompt'>();
  const route = mockRoute('HostKeyPrompt', {
    sshId: 'ssh-race',
    serverId: server.serverId,
    fp: 'SHA256:new-fingerprint',
    keyType: 'ED25519',
    kind: 'changed',
    oldFp: 'SHA256:old-fingerprint',
  });

  await renderWithProviders(
    <HostKeyPromptScreen navigation={navigation} route={route} />,
    { core, platform: createFakePlatform() },
  );
  await waitFor(() => {
    expect(screen.getByTestId('host-key-prompt')).toBeOnTheScreen();
  });

  await act(async () => {
    core.emit({
      type: 'ssh.state',
      sshId: 'ssh-race',
      serverId: server.serverId,
      state: 'failed',
    });
  });

  expect(navigation.goBack).not.toHaveBeenCalled();
  expect(screen.getByTestId('host-key-prompt')).toBeOnTheScreen();
});

it('still closes a changed-key prompt when the SSH connection is closed', async () => {
  const core = createFakeCore();
  const navigation = mockNavigation<'HostKeyPrompt'>();
  const route = mockRoute('HostKeyPrompt', {
    sshId: 'ssh-race',
    serverId: server.serverId,
    fp: 'SHA256:new-fingerprint',
    keyType: 'ED25519',
    kind: 'changed',
    oldFp: 'SHA256:old-fingerprint',
  });

  await renderWithProviders(
    <HostKeyPromptScreen navigation={navigation} route={route} />,
    { core, platform: createFakePlatform() },
  );
  await waitFor(() => {
    expect(screen.getByTestId('host-key-prompt')).toBeOnTheScreen();
  });

  await act(async () => {
    core.emit({
      type: 'ssh.state',
      sshId: 'ssh-race',
      serverId: server.serverId,
      state: 'closed',
    });
  });

  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});
