import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { StackActions } from '@react-navigation/native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore, type FakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { failLocalSync, tailnetPeer } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { ToastHost, hideToast } from '../../components/ToastHost';
import { installFakeAdapters } from '../adapters/testing/fakeAdapters';
import { localSyncGet } from '../data/localSync';
import type { KvStore } from '../storage/kv';
import type { ManualComputer, ServersValue } from '../servers/manual';
import NativeSheetScreen from './NativeSheet';

jest.setTimeout(30_000);

// useConnect navigates through the root ref.
const mockRootNavigate = jest.fn();
const mockRootDispatch = jest.fn();
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    isReady: () => true,
    navigate: (...args: unknown[]) => mockRootNavigate(...args),
    dispatch: (...args: unknown[]) => mockRootDispatch(...args),
  },
}));

// The local directory names a tailnet computer after its node.
const officeMac = tailnetPeer({
  hostname: 'office-mac',
  ips: ['fd7a:115c::40', '100.64.0.40'],
  os: 'macOS',
});

const labPc: ManualComputer = {
  id: 'lab-pc',
  kind: 'computer',
  host: '192.168.31.50',
  port: 3389,
  hostId: null,
  name: 'Lab PC',
  networkId: null,
  addedAtUs: FIXTURE_NOW,
  os: 'windows',
  protocol: 'rdp',
};

function sheetCore(): FakeCore {
  return createFakeCore();
}

/** The servers namespace saved on this phone. */
function savedServers(kv: KvStore) {
  return localSyncGet<ServersValue>(kv, 'servers');
}

async function renderSheet(
  entryKey: string,
  core: FakeCore = sheetCore(),
  platform?: FakePlatform,
) {
  const navigation = mockNavigation<'NativeSheet'>();
  const route: RootScreenProps<'NativeSheet'>['route'] = mockRoute(
    'NativeSheet',
    { entryKey },
  );
  const rendered = await renderWithProviders(
    <NativeSheetScreen navigation={navigation} route={route} />,
    {
      core,
      platform,
      // Tailnet computers are only listed with 外网访问 on.
      settings: { 'tailnet.enabled': true },
      localDirectory: { manual: [labPc], peers: [officeMac] },
    },
  );
  return { ...rendered, navigation };
}

afterEach(async () => {
  // The global toast lives in module state; do not let it leak.
  await act(async () => hideToast());
  await cleanup();
  mockRootNavigate.mockReset();
  mockRootDispatch.mockReset();
  jest.restoreAllMocks();
});

it('shows a tailnet Mac and connects through screen sharing', async () => {
  const core = sheetCore();
  installFakeAdapters(core);
  await renderSheet('tailnet:office-mac', core);

  expect(await screen.findByText('office-mac')).toBeOnTheScreen();
  expect(screen.getByText('Mac · 屏幕共享')).toBeOnTheScreen();
  expect(screen.getByText('系统共享')).toBeOnTheScreen();
  expect(screen.getByLabelText('系统，macOS')).toBeOnTheScreen();
  expect(screen.getByLabelText('地址，100.64.0.40')).toBeOnTheScreen();
  expect(screen.getByLabelText('在线状态，在线')).toBeOnTheScreen();

  await fireEvent.press(screen.getByTestId('native-sheet-connect'));
  // The session replaces the sheet, so ending it returns to Home.
  await waitFor(() =>
    expect(mockRootDispatch).toHaveBeenCalledWith(
      StackActions.replace('AdapterSession', {
        target: { protocol: 'vnc', addr: '100.64.0.40' },
        name: 'office-mac',
        addrKind: 'tailnet',
        context: 'native',
      }),
    ),
  );
  expect(mockRootDispatch.mock.calls[0][0].type).toBe('REPLACE');
  expect(mockRootNavigate).not.toHaveBeenCalled();
});

it('shows the external-app guide on the sheet when the app is missing', async () => {
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  await renderSheet('tailnet:office-mac', sheetCore(), platform);

  expect(await screen.findByText('office-mac')).toBeOnTheScreen();
  expect(screen.queryByTestId('fallback-guide-card')).toBeNull();
  await fireEvent.press(screen.getByTestId('native-sheet-connect'));

  const guide = await screen.findByTestId('fallback-guide-card');
  expect(within(guide).getByText('需要一个屏幕共享 App')).toBeOnTheScreen();
  expect(screen.getByTestId('native-sheet')).toBeOnTheScreen();
  expect(mockRootNavigate).not.toHaveBeenCalled();
  expect(mockRootDispatch).not.toHaveBeenCalled();

  await fireEvent.press(within(guide).getByRole('button', { name: '取消' }));
  expect(screen.queryByTestId('fallback-guide-card')).toBeNull();
});

it('offers no rename or delete for a tailnet computer', async () => {
  await renderSheet('tailnet:office-mac');

  expect(await screen.findByText('office-mac')).toBeOnTheScreen();
  expect(screen.queryByTestId('native-sheet-rename')).toBeNull();
  expect(screen.queryByTestId('native-sheet-remove')).toBeNull();
  expect(screen.queryByText('名字和地址')).toBeNull();
  expect(screen.queryByText('删除这台电脑')).toBeNull();
});

it('shows a manual PC, probes its port and connects through Remote Desktop', async () => {
  const core = sheetCore();
  installFakeAdapters(core);
  await renderSheet('manual:lab-pc', core);

  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();
  expect(screen.getByLabelText('系统，Windows')).toBeOnTheScreen();
  expect(screen.getByLabelText('地址，192.168.31.50')).toBeOnTheScreen();
  expect(
    screen.getByLabelText('名字和地址，192.168.31.50，Lab PC'),
  ).toBeOnTheScreen();
  expect(await screen.findByLabelText('在线状态，离线')).toBeOnTheScreen();
  expect(core.calls).toContainEqual({
    method: 'discover.probe',
    args: ['192.168.31.50', 3389, 'rdp', 800],
  });

  await fireEvent.press(screen.getByTestId('native-sheet-connect'));
  await waitFor(() =>
    expect(mockRootDispatch).toHaveBeenCalledWith(
      StackActions.replace('AdapterSession', {
        target: { protocol: 'rdp', addr: '192.168.31.50' },
        name: 'Lab PC',
        addrKind: 'manual',
        context: 'native',
      }),
    ),
  );
  expect(mockRootNavigate).not.toHaveBeenCalled();
});

it('says the reach is unknown while the first probe is pending', async () => {
  const core = sheetCore();
  jest
    .spyOn(core.discover!, 'probe')
    .mockReturnValue(new Promise(() => undefined));
  await renderSheet('manual:lab-pc', core);

  expect(
    await screen.findByLabelText('在线状态，在线状态未知'),
  ).toBeOnTheScreen();
  expect(screen.getAllByText('检测中…').length).toBeGreaterThan(0);
  expect(screen.queryByText('离线')).toBeNull();
});

it('renames a manual PC in the servers saved on this phone', async () => {
  const { kv } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();

  await fireEvent.press(screen.getByTestId('native-sheet-rename'));
  const dialog = screen.getByTestId('native-sheet-rename-dialog-card');
  const field = within(dialog).getByTestId('native-sheet-name');
  expect(field).toHaveProp('value', 'Lab PC');
  await fireEvent.changeText(field, '  实验室 PC  ');
  await fireEvent.press(within(dialog).getByText('保存'));

  await waitFor(() => expect(savedServers(kv)?.version).toBe(2));
  expect(savedServers(kv)?.value.entries).toEqual([
    { ...labPc, name: '实验室 PC' },
  ]);
  expect(
    await screen.findByLabelText('名字和地址，192.168.31.50，实验室 PC'),
  ).toBeOnTheScreen();
});

it('changes the address and port of a manual PC', async () => {
  const { kv } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();

  await fireEvent.press(screen.getByTestId('native-sheet-rename'));
  const dialog = screen.getByTestId('native-sheet-rename-dialog-card');
  expect(within(dialog).getByTestId('native-sheet-host')).toHaveProp(
    'value',
    '192.168.31.50',
  );
  expect(within(dialog).getByTestId('native-sheet-port')).toHaveProp(
    'value',
    '3389',
  );
  await fireEvent.changeText(
    within(dialog).getByTestId('native-sheet-host'),
    '10.30.12.60:3390',
  );
  await fireEvent.changeText(within(dialog).getByTestId('native-sheet-port'), '');
  await fireEvent.press(within(dialog).getByText('保存'));

  await waitFor(() => expect(savedServers(kv)?.version).toBe(2));
  expect(savedServers(kv)?.value.entries).toEqual([
    { ...labPc, host: '10.30.12.60', port: 3390 },
  ]);
  expect(
    await screen.findByLabelText('名字和地址，10.30.12.60:3390，Lab PC'),
  ).toBeOnTheScreen();
});

it('switches a manual PC between Remote Desktop and Screen Sharing', async () => {
  const { kv } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();
  expect(screen.getByTestId('native-sheet-protocol-rdp')).toBeSelected();
  expect(screen.getByTestId('native-sheet-protocol-vnc')).not.toBeSelected();

  // A saved Windows PC that answers on 5900 too can be moved to VNC…
  await fireEvent.press(screen.getByRole('radio', { name: '屏幕共享 VNC' }));
  await waitFor(() => expect(savedServers(kv)?.version).toBe(2));
  expect(savedServers(kv)?.value.entries).toEqual([
    { ...labPc, os: 'macos', protocol: 'vnc', port: 5900 },
  ]);
  expect(await screen.findByText('Mac · 屏幕共享')).toBeOnTheScreen();
  expect(screen.getByTestId('native-sheet-protocol-vnc')).toBeSelected();

  // …and back, with the default port following the protocol.
  await fireEvent.press(screen.getByRole('radio', { name: '远程桌面 RDP' }));
  await waitFor(() => expect(savedServers(kv)?.version).toBe(3));
  expect(savedServers(kv)?.value.entries).toEqual([labPc]);
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();
});

it('keeps a custom port when switching the connection method', async () => {
  const { kv } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('native-sheet-rename'));
  const dialog = screen.getByTestId('native-sheet-rename-dialog-card');
  await fireEvent.changeText(
    within(dialog).getByTestId('native-sheet-port'),
    '3390',
  );
  await fireEvent.press(within(dialog).getByText('保存'));
  await waitFor(() => expect(savedServers(kv)?.version).toBe(2));

  await fireEvent.press(screen.getByTestId('native-sheet-protocol-vnc'));
  await waitFor(() => expect(savedServers(kv)?.version).toBe(3));
  expect(savedServers(kv)?.value.entries).toEqual([
    { ...labPc, os: 'macos', protocol: 'vnc', port: 3390 },
  ]);
});

it('offers no connection method switch for a tailnet computer', async () => {
  await renderSheet('tailnet:office-mac');
  expect(await screen.findByText('office-mac')).toBeOnTheScreen();
  expect(screen.queryByTestId('native-sheet-protocol')).toBeNull();
});

it('keeps the edit dialog open on a bad address or port', async () => {
  const { kv } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('native-sheet-rename'));
  const dialog = () => screen.getByTestId('native-sheet-rename-dialog-card');

  await fireEvent.changeText(
    within(dialog()).getByTestId('native-sheet-host'),
    'a b',
  );
  await fireEvent.press(within(dialog()).getByText('保存'));
  expect(within(dialog()).getByText('不像是 IP 或主机名')).toBeOnTheScreen();

  await fireEvent.changeText(
    within(dialog()).getByTestId('native-sheet-host'),
    'lab-pc',
  );
  await fireEvent.changeText(
    within(dialog()).getByTestId('native-sheet-port'),
    '70000',
  );
  await fireEvent.press(within(dialog()).getByText('保存'));
  expect(
    within(dialog()).getByText('端口是 1 到 65535 的数字'),
  ).toBeOnTheScreen();
  expect(savedServers(kv)?.version).toBe(1);
});

it('deletes a manual PC after confirmation and closes the sheet', async () => {
  const { kv, navigation } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();

  await fireEvent.press(screen.getByTestId('native-sheet-remove'));
  expect(screen.getByText('删除 Lab PC？')).toBeOnTheScreen();
  const dialog = screen.getByTestId('native-sheet-remove-dialog-card');
  await fireEvent.press(within(dialog).getByText('删除'));

  await waitFor(() => expect(navigation.goBack).toHaveBeenCalledTimes(1));
  expect(savedServers(kv)?.version).toBe(2);
  expect(
    savedServers(kv)?.value.entries.some(entry => entry.id === 'lab-pc'),
  ).toBe(false);
});

it('offers to undo the delete from the App-wide toast', async () => {
  const navigation = mockNavigation<'NativeSheet'>();
  const route: RootScreenProps<'NativeSheet'>['route'] = mockRoute(
    'NativeSheet',
    { entryKey: 'manual:lab-pc' },
  );
  const { kv } = await renderWithProviders(
    <>
      <NativeSheetScreen navigation={navigation} route={route} />
      <ToastHost />
    </>,
    { localDirectory: { manual: [labPc], peers: [officeMac] } },
  );
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();

  await fireEvent.press(screen.getByTestId('native-sheet-remove'));
  const dialog = screen.getByTestId('native-sheet-remove-dialog-card');
  await fireEvent.press(within(dialog).getByText('删除'));
  await waitFor(() => expect(navigation.goBack).toHaveBeenCalledTimes(1));
  // The mocked goBack leaves the modal sheet up, which hides the App-wide
  // toast from the accessibility tree; in the App the sheet is gone by now.
  const hidden = { includeHiddenElements: true };
  expect(await screen.findByText('已删除 Lab PC', hidden)).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('撤销', hidden));
  await waitFor(() =>
    expect(
      savedServers(kv)?.value.entries.some(entry => entry.id === 'lab-pc'),
    ).toBe(true),
  );
});

it('keeps the sheet open with a toast when deleting fails', async () => {
  const { kv, navigation } = await renderSheet('manual:lab-pc');
  expect(await screen.findByText('Windows · 远程桌面')).toBeOnTheScreen();
  failLocalSync(kv, 'servers', new Error('put failed'));

  await fireEvent.press(screen.getByTestId('native-sheet-remove'));
  const dialog = screen.getByTestId('native-sheet-remove-dialog-card');
  await fireEvent.press(within(dialog).getByText('删除'));

  expect(await screen.findByText('操作失败，稍后再试')).toBeOnTheScreen();
  expect(navigation.goBack).not.toHaveBeenCalled();
  expect(screen.getByTestId('native-sheet-remove')).toBeOnTheScreen();
});

it('links to the optional Remote Host install guide', async () => {
  const { navigation } = await renderSheet('tailnet:office-mac');
  expect(await screen.findByText('office-mac')).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('安装 Host（可选）'));
  expect(navigation.replace).toHaveBeenCalledWith('PairAdd');
});

it('shows a not-found state for an unknown computer', async () => {
  const { navigation } = await renderSheet('manual:missing');

  expect(await screen.findByText('找不到这台电脑')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('关闭'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});
