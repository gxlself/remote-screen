import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import AddServerScreen from '.';
import type {
  RootScreenProps,
  RootStackParamList,
} from '../../app/navigation/routes';
import { localSyncGet } from '../../features/data/localSync';
import type { ServersValue } from '../../features/servers/manual';
import type { KvStore } from '../../features/storage/kv';
import { addServer } from '../../i18n/zh/addServer';
import { failLocalSync } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';

const PREFILL = { name: 'aliyun-bj', host: '100.64.0.60', port: 22 };
/** What AddByAddress passes for an address that only answers on 22. */
const FROM_ADDRESS = {
  name: '10.30.99.9',
  host: '10.30.99.9',
  port: 22,
  connectAfterSave: true,
};

const clients: QueryClient[] = [];

async function mount(params: RootStackParamList['AddServer']) {
  const screenProps = {
    navigation: mockNavigation<'AddServer'>(),
    route: mockRoute('AddServer', params),
  } as RootScreenProps<'AddServer'>;
  const view = await renderWithProviders(<AddServerScreen {...screenProps} />);
  clients.push(view.queryClient);
  await act(async () => {
    await Promise.resolve();
  });
  return { ...view, ...screenProps };
}

/** The servers namespace saved on this phone, if anything was saved. */
function manualValue(kv: KvStore) {
  return localSyncGet<ServersValue>(kv, 'servers')?.value;
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('fills name, address and port from the prefill and saves a manual server', async () => {
  const { kv, navigation } = await mount({ prefill: PREFILL });

  expect(await screen.findByText(addServer.title.add)).toBeOnTheScreen();
  expect(screen.getByLabelText('名称')).toHaveProp('value', 'aliyun-bj');
  expect(screen.getByLabelText('地址')).toHaveProp('value', '100.64.0.60');
  expect(screen.getByLabelText('端口')).toHaveProp('value', '22');
  expect(screen.getByLabelText('用户名')).toHaveProp('value', '');

  await fireEvent.changeText(screen.getByLabelText('用户名'), 'root');
  await fireEvent.press(screen.getByRole('button', { name: '保存' }));
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  expect(manualValue(kv)?.entries).toEqual([
    expect.objectContaining({
      kind: 'ssh',
      name: 'aliyun-bj',
      host: '100.64.0.60',
      port: 22,
      user: 'root',
      auth: 'deviceKey',
      jump: null,
    }),
  ]);
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('takes a prefilled port other than 22', async () => {
  await mount({ prefill: { ...PREFILL, port: 2222 } });

  expect(await screen.findByLabelText('端口')).toHaveProp('value', '2222');
});

it('edits the server by id and ignores the prefill', async () => {
  await mount({ serverId: 'manual-build-srv', prefill: PREFILL });

  expect(await screen.findByText(addServer.title.edit)).toBeOnTheScreen();
  expect(screen.getByLabelText('名称')).toHaveProp('value', 'build-srv');
  expect(screen.getByLabelText('地址')).toHaveProp('value', '10.30.20.31');
  expect(screen.getByLabelText('用户名')).toHaveProp('value', 'alice');
});

it('focuses the user field when the name and address are prefilled', async () => {
  await mount({ prefill: PREFILL });

  expect(await screen.findByLabelText('用户名')).toHaveProp('autoFocus', true);
});

it('does not focus the user field on a blank form', async () => {
  await mount(undefined);

  expect(await screen.findByLabelText('用户名')).toHaveProp('autoFocus', false);
});

it('saves and opens the terminal the way Home does with connectAfterSave', async () => {
  const { kv, navigation } = await mount({ prefill: FROM_ADDRESS });

  expect(await screen.findByLabelText('用户名')).toHaveProp('autoFocus', true);
  expect(screen.getByLabelText('名称')).toHaveProp('value', '10.30.99.9');
  expect(screen.getByLabelText('地址')).toHaveProp('value', '10.30.99.9');
  expect(screen.getByLabelText('端口')).toHaveProp('value', '22');
  expect(screen.queryByRole('button', { name: '保存' })).toBeNull();

  await fireEvent.changeText(screen.getByLabelText('用户名'), 'root');
  await fireEvent.press(
    screen.getByRole('button', { name: addServer.actions.saveAndConnect }),
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });

  expect(manualValue(kv)?.entries).toEqual([
    expect.objectContaining({
      id: 'manual-10-30-99-9',
      kind: 'ssh',
      name: '10.30.99.9',
      host: '10.30.99.9',
      port: 22,
      user: 'root',
      auth: 'deviceKey',
      jump: null,
    }),
  ]);
  // Home opens a server with ('Terminal', { serverId }); replace drops the form.
  expect(navigation.replace).toHaveBeenCalledTimes(1);
  expect(navigation.replace).toHaveBeenCalledWith('Terminal', {
    serverId: 'manual-10-30-99-9',
  });
  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(navigation.goBack).not.toHaveBeenCalled();
});

it('stays on the form and does not connect when the save fails', async () => {
  const { kv, navigation } = await mount({ prefill: FROM_ADDRESS });
  failLocalSync(kv, 'servers', new Error('offline'));

  await fireEvent.changeText(await screen.findByLabelText('用户名'), 'root');
  await fireEvent.press(screen.getByRole('button', { name: '保存并连接' }));
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });

  expect(screen.getByTestId('save-error')).toHaveTextContent(
    addServer.save.failed,
  );
  expect(navigation.replace).not.toHaveBeenCalled();
  expect(navigation.goBack).not.toHaveBeenCalled();
});
