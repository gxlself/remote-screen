import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { StyleSheet } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { SyncValue } from '../../native/rscore/types';
import { localSyncGet } from '../../features/data/localSync';
import { MemoryKv } from '../../features/storage/kv';
import { failLocalSync, seedLocalSync } from '../../test/localDirectory';
import ToastHost, { hideToast } from '../../components/ToastHost';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import * as networksModule from '../../features/network/useNetworks';
import NetworksScreen from './index';

const queryClients: QueryClient[] = [];

const existing = {
  id: 'net-1',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

/** `saved` is the networks namespace already on this phone. */
async function renderScreen(saved?: SyncValue, setup?: (kv: MemoryKv) => void) {
  const core = createFakeCore();
  const kv = new MemoryKv();
  if (saved !== undefined) {
    seedLocalSync(kv, 'networks', saved);
  }
  setup?.(kv);
  const navigation = mockNavigation<'Networks'>();
  const result = await renderWithProviders(
    <>
      <NetworksScreen navigation={navigation} route={mockRoute('Networks')} />
      <ToastHost />
    </>,
    { core, kv, localDirectory: {} },
  );
  queryClients.push(result.queryClient);
  await waitFor(() =>
    expect(core.calls.some(call => call.method === 'hosts.paired')).toBe(true),
  );
  await act(() => Promise.resolve());
  return { ...result, core, kv, navigation };
}

afterEach(async () => {
  await act(async () => hideToast());
  jest.restoreAllMocks();
  await act(() => Promise.resolve());
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
});

it('shows the empty state without host suggestions', async () => {
  await renderScreen();
  expect(await screen.findByText('还没有登记网络')).toBeOnTheScreen();
  // Hosts reported their subnets only through a Hub.
  expect(screen.queryByText('工位 Windows 上报')).toBeNull();
  expect(screen.queryByLabelText('添加 10.30.0.0/16')).toBeNull();
});

it('adds a network, validates its name, and saves it on this phone', async () => {
  const { kv, settingsStore } = await renderScreen();
  await fireEvent.press(await screen.findByLabelText('添加其他网络'));
  expect(screen.getByText('添加网络')).toBeOnTheScreen();
  expect(
    StyleSheet.flatten(screen.getByTestId('network-cidrs').props.style)
      .fontFamily,
  ).toBe('GeistMono-Regular');
  await fireEvent.changeText(
    screen.getByTestId('network-cidrs'),
    '10.30.0.0/16',
  );

  await fireEvent.press(screen.getByLabelText('保存'));
  expect(screen.getByText('必填')).toBeOnTheScreen();

  await fireEvent.changeText(screen.getByTestId('network-name'), '公司网络');
  await fireEvent.changeText(
    screen.getByTestId('network-vpn-pools'),
    '10.99.0.0/16',
  );
  await fireEvent.press(screen.getByLabelText('保存'));
  await waitFor(() =>
    expect(localSyncGet(kv, 'networks')).toEqual({
      version: 1,
      value: {
        v: 1,
        items: [
          {
            id: 'net-1',
            name: '公司网络',
            cidrs: ['10.30.0.0/16'],
            vpnPools: ['10.99.0.0/16'],
          },
        ],
      },
    }),
  );

  await waitFor(() => expect(screen.getByText('公司网络')).toBeOnTheScreen());
  expect(settingsStore.get('networks')).toEqual([existing]);
  // S28: the save says it landed.
  expect(screen.getByTestId('global-toast')).toHaveTextContent('已保存');
});

it('offers a numeric keyboard for the subnet fields', async () => {
  await renderScreen();
  await fireEvent.press(await screen.findByLabelText('添加其他网络'));
  // S27
  expect(screen.getByTestId('network-cidrs')).toHaveProp(
    'keyboardType',
    'numbers-and-punctuation',
  );
  expect(screen.getByTestId('network-vpn-pools')).toHaveProp(
    'keyboardType',
    'numbers-and-punctuation',
  );
});

it('reports a failed delete with delete wording', async () => {
  await renderScreen({ version: 1, value: { v: 1, items: [existing] } }, kv =>
    failLocalSync(kv, 'networks', new Error('磁盘满了')),
  );
  await fireEvent.press(await screen.findByLabelText('编辑 公司网络'));
  await fireEvent.press(screen.getByLabelText('删除网络 公司网络'));
  await fireEvent.press(
    screen.getByTestId('delete-network-confirm', {
      includeHiddenElements: true,
    }),
  );
  // S27: not the save-failure copy.
  await waitFor(() =>
    expect(
      screen.getByText('没删掉：磁盘满了', { includeHiddenElements: true }),
    ).toBeOnTheScreen(),
  );
  expect(
    screen.queryByText(/没保存上/, { includeHiddenElements: true }),
  ).toBeNull();
});

it('reports a bad VPN pool CIDR', async () => {
  await renderScreen();
  await fireEvent.press(screen.getByLabelText('添加其他网络'));
  await fireEvent.changeText(screen.getByTestId('network-name'), '公司网络');
  await fireEvent.changeText(
    screen.getByTestId('network-cidrs'),
    '10.30.0.0/16',
  );
  await fireEvent.changeText(
    screen.getByTestId('network-vpn-pools'),
    '10.99.0.0/99',
  );
  await fireEvent.press(screen.getByLabelText('保存'));
  expect(screen.getByText('网段格式不对：10.99.0.0/99')).toBeOnTheScreen();
});

it('edits and deletes an existing network after confirmation', async () => {
  await renderScreen({ version: 1, value: { v: 1, items: [existing] } });
  await fireEvent.press(await screen.findByLabelText('编辑 公司网络'));
  expect(screen.getByText('编辑网络')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('删除网络 公司网络'));
  expect(
    screen.getByTestId('delete-network-dialog-card', {
      includeHiddenElements: true,
    }),
  ).toBeTruthy();
  await fireEvent.press(
    screen.getByLabelText('删除', { includeHiddenElements: true }),
  );
  await waitFor(() =>
    expect(screen.getByText('还没有登记网络')).toBeOnTheScreen(),
  );
  expect(screen.getByTestId('global-toast')).toHaveTextContent('已删除');
});

it('shows progress while the list loads instead of a blank block', async () => {
  jest.spyOn(networksModule, 'useNetworks').mockReturnValue({
    items: [],
    isLoading: true,
    save: jest.fn(),
    remove: jest.fn(),
  } as unknown as ReturnType<typeof networksModule.useNetworks>);
  const result = await renderWithProviders(
    <NetworksScreen
      navigation={mockNavigation<'Networks'>()}
      route={mockRoute('Networks')}
    />,
    { core: createFakeCore(), kv: new MemoryKv(), localDirectory: {} },
  );
  queryClients.push(result.queryClient);
  // S28
  expect(screen.getByTestId('networks-loading')).toBeOnTheScreen();
  expect(screen.queryByText('还没有登记网络')).toBeNull();
});

it('goes back to settings', async () => {
  const { navigation } = await renderScreen();
  await fireEvent.press(screen.getByLabelText('返回设置'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});
