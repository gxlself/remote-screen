import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { sshServers } from '../../native/rscore/fixtures';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import HostKeyPromptScreen from './index';

jest.setTimeout(20_000);

const gpu = sshServers.find(server => server.name === 'gpu-train-02');
if (gpu === undefined) {
  throw new Error('gpu-train-02 fixture missing');
}
const fp = 'SHA256:nT4kP0aZWq8c';

function renderPrompt(
  params: {
    kind: 'first' | 'changed';
    oldFp?: string;
  } = { kind: 'first' },
) {
  const core = createFakeCore();
  const platform = createFakePlatform();
  const navigation = mockNavigation<'HostKeyPrompt'>();
  const route = mockRoute('HostKeyPrompt', {
    sshId: 'ssh1',
    serverId: gpu!.serverId,
    fp,
    keyType: 'ED25519',
    ...params,
  });
  return {
    core,
    platform,
    navigation,
    render: async () => {
      const result = await renderWithProviders(
        <HostKeyPromptScreen navigation={navigation} route={route} />,
        { core, platform },
      );
      await waitFor(() => {
        expect(
          screen.getByText(
            params.kind === 'changed'
              ? 'gpu-train-02 的主机指纹变了'
              : '第一次连接 gpu-train-02',
          ),
        ).toBeOnTheScreen();
      });
      return result;
    },
  };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('shows the first-connection copy and trusts the fingerprint without Face ID', async () => {
  const stub = renderPrompt();
  await stub.render();

  expect(screen.getByText('第一次连接 gpu-train-02')).toBeOnTheScreen();
  expect(screen.getByLabelText(/^主机指纹 ED25519，/)).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('信任并连接'));

  await waitFor(() => {
    expect(stub.core.calls).toContainEqual({
      method: 'ssh.trustHostKey',
      args: [gpu!.serverId, fp],
    });
  });
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
  expect(
    stub.platform.calls.filter(call => call.method === 'authenticate'),
  ).toHaveLength(0);
});

it('closes the SSH connection when cancelling a first-connection prompt', async () => {
  const stub = renderPrompt();
  await stub.render();

  await fireEvent.press(screen.getByText('取消'));

  expect(stub.core.calls).toContainEqual({
    method: 'ssh.close',
    args: ['ssh1'],
  });
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('shows both fingerprints and disconnects by default after a fingerprint change', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  await stub.render();

  expect(screen.getByText('原来')).toBeOnTheScreen();
  expect(screen.getByText('现在')).toBeOnTheScreen();
  expect(screen.getByTestId('host-key-old')).toBeOnTheScreen();
  expect(screen.getByTestId('host-key-current')).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('断开'));

  expect(stub.core.calls).toContainEqual({
    method: 'ssh.close',
    args: ['ssh1'],
  });
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('requires Face ID before trusting a changed fingerprint', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  await stub.render();

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));

  await waitFor(() => {
    expect(stub.core.calls).toContainEqual({
      method: 'ssh.trustHostKey',
      args: [gpu!.serverId, fp],
    });
  });
  const trustIndex = stub.core.calls.findIndex(
    call => call.method === 'ssh.trustHostKey',
  );
  const authIndex = stub.platform.calls.findIndex(
    call => call.method === 'authenticate',
  );
  expect(authIndex).toBeGreaterThanOrEqual(0);
  expect(trustIndex).toBeGreaterThanOrEqual(0);
  expect(stub.platform.calls[authIndex]).toEqual({
    method: 'authenticate',
    args: ['确认信任新的主机指纹'],
  });
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('stays on the prompt when Face ID is cancelled', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  stub.platform.authResult = 'cancelled';
  await stub.render();

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));

  await waitFor(() => {
    expect(
      stub.platform.calls.filter(call => call.method === 'authenticate'),
    ).toHaveLength(1);
  });
  expect(
    stub.core.calls.filter(call => call.method === 'ssh.trustHostKey'),
  ).toHaveLength(0);
  expect(stub.navigation.goBack).not.toHaveBeenCalled();
});

it('shows the save error and stays open when trusting the new key fails', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  stub.core.failNext('ssh.trustHostKey', new Error('disk full'));
  await stub.render();

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));

  await waitFor(() => {
    expect(screen.getByText('没能保存指纹：disk full')).toBeOnTheScreen();
  });
  expect(stub.navigation.goBack).not.toHaveBeenCalled();
});

it('closes the prompt when the backdrop is pressed', async () => {
  const stub = renderPrompt();
  await stub.render();

  await fireEvent.press(screen.getByTestId('host-key-prompt-backdrop'));

  expect(stub.core.calls).toContainEqual({
    method: 'ssh.close',
    args: ['ssh1'],
  });
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('closes the prompt when the connection ends while it is open', async () => {
  const stub = renderPrompt();
  await stub.render();

  await act(async () => {
    stub.core.emit({
      type: 'ssh.state',
      sshId: 'ssh1',
      serverId: gpu!.serverId,
      state: 'closed',
    });
  });

  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('says the change was not trusted when Face ID fails', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  stub.platform.authResult = 'failed';
  await stub.render();

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));

  // T08: a failed check says so instead of doing nothing.
  await waitFor(() =>
    expect(screen.getByText('没有验证通过，新指纹没有保存')).toBeOnTheScreen(),
  );
  expect(
    stub.core.calls.filter(call => call.method === 'ssh.trustHostKey'),
  ).toHaveLength(0);
  expect(stub.navigation.goBack).not.toHaveBeenCalled();
});

it('asks once more before trusting a change on a phone without a lock', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  stub.platform.authResult = 'unavailable';
  await stub.render();
  const hidden = { includeHiddenElements: true };

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));

  // T08: no lock is not a pass.
  await waitFor(() =>
    expect(
      screen.getByTestId('host-key-no-lock-dialog', hidden),
    ).toBeOnTheScreen(),
  );
  expect(
    stub.core.calls.filter(call => call.method === 'ssh.trustHostKey'),
  ).toHaveLength(0);
  await fireEvent.press(screen.getByTestId('host-key-no-lock-confirm', hidden));
  await waitFor(() =>
    expect(stub.core.calls).toContainEqual({
      method: 'ssh.trustHostKey',
      args: [gpu!.serverId, fp],
    }),
  );
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('keeps the old key when the no-lock confirmation is cancelled', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  stub.platform.authResult = 'unavailable';
  await stub.render();
  const hidden = { includeHiddenElements: true };

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));
  await waitFor(() =>
    expect(
      screen.getByTestId('host-key-no-lock-dialog', hidden),
    ).toBeOnTheScreen(),
  );
  await fireEvent.press(screen.getAllByText('取消', hidden).at(-1)!);
  expect(
    stub.core.calls.filter(call => call.method === 'ssh.trustHostKey'),
  ).toHaveLength(0);
  expect(stub.navigation.goBack).not.toHaveBeenCalled();
});

it('says when the original fingerprint was not recorded', async () => {
  const stub = renderPrompt({ kind: 'changed' });
  await stub.render();
  expect(screen.getByTestId('host-key-old')).toHaveTextContent(
    /没有记下原来的指纹/,
  );
});

it('shows the fingerprint as one selectable text with its key type', async () => {
  const stub = renderPrompt();
  await stub.render();
  // T18: the fingerprint can be selected and copied in one go.
  const block = within(screen.getByTestId('host-key-fingerprint'));
  const selectable = block
    .getAllByText(/./)
    .filter(node => node.props.selectable === true);
  expect(selectable).toHaveLength(1);
  expect(block.getByText('ED25519')).toBeOnTheScreen();
  expect(screen.getByText(/ssh-keygen -lf/)).toBeOnTheScreen();
  expect(screen.queryByText(/管理员/)).toBeNull();
});

it('holds the disconnect while the new key is being saved', async () => {
  const stub = renderPrompt({ kind: 'changed', oldFp: 'SHA256:old-old-old' });
  let finish: () => void = () => undefined;
  jest.spyOn(stub.core.ssh, 'trustHostKey').mockImplementation(
    () =>
      new Promise<void>(resolve => {
        finish = resolve;
      }),
  );
  await stub.render();

  await fireEvent.press(screen.getByText('我确认换过了，信任新指纹'));
  // T19
  await waitFor(() =>
    expect(screen.getByRole('button', { name: '断开' })).toBeDisabled(),
  );
  await fireEvent.press(screen.getByTestId('host-key-prompt-backdrop'));
  expect(
    stub.core.calls.filter(call => call.method === 'ssh.close'),
  ).toHaveLength(0);
  await act(async () => finish());
  await waitFor(() => expect(stub.navigation.goBack).toHaveBeenCalledTimes(1));
});
