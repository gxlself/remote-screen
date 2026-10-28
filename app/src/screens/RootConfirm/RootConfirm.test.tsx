import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { sshServers } from '../../native/rscore/fixtures';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import RootConfirmScreen from './index';

jest.setTimeout(20_000);

const prod = sshServers.find(server => server.name === 'prod-api-01');
if (prod === undefined) {
  throw new Error('prod-api-01 fixture missing');
}
const url = 'https://hs.example.com/a/xyz';

function renderConfirm() {
  const core = createFakeCore();
  const platform = createFakePlatform();
  const navigation = mockNavigation<'RootConfirm'>();
  const route = mockRoute('RootConfirm', {
    sshId: 'ssh1',
    serverId: prod!.serverId,
    url,
    user: 'root',
  });
  return {
    core,
    platform,
    navigation,
    render: async () => {
      const result = await renderWithProviders(
        <RootConfirmScreen navigation={navigation} route={route} />,
        { core, platform },
      );
      await waitFor(() => {
        expect(
          screen.getByText('以 root 登录 prod-api-01 需要再确认一次'),
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

it('shows the request summary and the three confirmation steps', async () => {
  const stub = renderConfirm();
  await stub.render();

  expect(
    screen.getByText('以 root 登录 prod-api-01 需要再确认一次'),
  ).toBeOnTheScreen();
  expect(screen.getByText('登录为')).toBeOnTheScreen();
  expect(screen.getByText('root@prod-api-01')).toBeOnTheScreen();
  expect(screen.getByText('用 Face ID 确认是你本人')).toBeOnTheScreen();
  expect(screen.getByText('在浏览器登录 hs.example.com')).toBeOnTheScreen();
  expect(screen.getByText('浏览器登录完成后会自动回到终端')).toBeOnTheScreen();
  expect(screen.getByText(stub.platform.deviceInfo().model)).toBeOnTheScreen();
});

it('names Touch ID in the confirmation step and button', async () => {
  const platform = createFakePlatform();
  platform.authCapabilityResult = { biometry: 'touchId', passcode: true };
  const core = createFakeCore();
  const navigation = mockNavigation<'RootConfirm'>();
  const route = mockRoute('RootConfirm', {
    sshId: 'ssh1',
    serverId: prod!.serverId,
    url,
    user: 'root',
  });
  await renderWithProviders(
    <RootConfirmScreen navigation={navigation} route={route} />,
    { core, platform },
  );

  expect(await screen.findByText('用 Touch ID 确认是你本人')).toBeOnTheScreen();
  expect(
    screen.getByText('用 Touch ID 确认，并在浏览器登录'),
  ).toBeOnTheScreen();
  const icon = screen.getByTestId('root-confirm-authenticate-icon', {
    includeHiddenElements: true,
  });
  expect(icon.queryAll(node => node.type === 'RNSVGRect')).toHaveLength(1);
});

it('authenticates, opens the browser, and advances to the wait phase', async () => {
  const stub = renderConfirm();
  await stub.render();

  await fireEvent.press(screen.getByText('用 Face ID 确认，并在浏览器登录'));

  await waitFor(() => {
    expect(stub.platform.opened).toEqual([url]);
  });
  expect(stub.platform.calls).toContainEqual({
    method: 'authenticate',
    args: ['确认以 root 登录 prod-api-01'],
  });
  expect(screen.getByText('Face ID 已确认')).toBeOnTheScreen();
  expect(
    screen.getByText('在浏览器登录 hs.example.com · 等你登录'),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('浏览器没打开？再打开一次'));
  await waitFor(() => {
    expect(stub.platform.opened).toHaveLength(2);
  });
});

it('does not open the browser when Face ID fails', async () => {
  const stub = renderConfirm();
  stub.platform.authResult = 'failed';
  await stub.render();

  await fireEvent.press(screen.getByText('用 Face ID 确认，并在浏览器登录'));

  await waitFor(() => {
    expect(
      stub.platform.calls.filter(call => call.method === 'authenticate'),
    ).toHaveLength(1);
  });
  expect(stub.platform.opened).toHaveLength(0);
  expect(screen.getByText('用 Face ID 确认是你本人')).toBeOnTheScreen();
});

it('offers a copy action when the browser cannot be opened', async () => {
  const stub = renderConfirm();
  stub.platform.openUrlResult = false;
  await stub.render();

  await fireEvent.press(screen.getByText('用 Face ID 确认，并在浏览器登录'));
  await waitFor(() => {
    expect(screen.getByText('复制链接')).toBeOnTheScreen();
  });

  await fireEvent.press(screen.getByText('复制链接'));
  await waitFor(() => {
    expect(stub.platform.copied).toEqual([url]);
  });
});

it('closes an ask-phase prompt as soon as the SSH connection opens', async () => {
  const stub = renderConfirm();
  await stub.render();

  await act(async () => {
    stub.core.emit({
      type: 'ssh.state',
      sshId: 'ssh1',
      serverId: prod!.serverId,
      state: 'open',
    });
  });

  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});

it('closes the connection and the prompt when cancelling', async () => {
  const stub = renderConfirm();
  await stub.render();

  await fireEvent.press(screen.getByText('取消'));

  expect(stub.core.calls).toContainEqual({
    method: 'ssh.close',
    args: ['ssh1'],
  });
  expect(stub.navigation.goBack).toHaveBeenCalledTimes(1);
});
