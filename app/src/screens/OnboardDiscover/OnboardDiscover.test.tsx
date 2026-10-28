import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import { focusManager } from '@tanstack/react-query';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { ManualComputer } from '../../features/servers/manual';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { tailnetPeer } from '../../test/localDirectory';
import type { LocalDirectorySeed } from '../../test/localDirectory';
import OnboardDiscoverScreen from './index';

const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';

const gxl = tailnetPeer({
  hostname: 'home-mac',
  os: 'macOS',
  ips: ['fd7a:115c:a1e0::30', '100.64.0.30'],
});
const sleepingPc = tailnetPeer({
  hostname: 'win-241',
  os: 'windows',
  ips: ['100.64.0.41'],
  online: false,
});
const gpuTrain = tailnetPeer({
  hostname: 'gpu-train-02',
  os: 'linux',
  ips: ['100.64.0.32'],
  tags: ['tag:server'],
});
const nasHome = tailnetPeer({
  hostname: 'nas-home',
  os: 'linux',
  ips: ['100.64.0.33'],
  tags: ['tag:server'],
});
const lanPc: ManualComputer & { os: 'windows' } = {
  id: 'addr-192-168-31-88-3389',
  kind: 'computer',
  host: '192.168.31.88',
  port: 3389,
  hostId: null,
  name: 'Meeting PC',
  networkId: null,
  addedAtUs: 1,
  os: 'windows',
  protocol: 'rdp',
};

type Options = {
  core?: ReturnType<typeof createFakeCore>;
  platform?: FakePlatform;
  /** What this phone knows; defaults to the paired Hosts and two servers. */
  localDirectory?: LocalDirectorySeed;
  /** 外网访问 (Tailscale); the tailnet rows below need it on. */
  tailnetEnabled?: boolean;
};

async function renderScreen(options: Options = {}) {
  const core = options.core ?? createFakeCore();
  const platform = options.platform ?? new FakePlatform();
  const navigation = mockNavigation<'OnboardDiscover'>();
  const result = await renderWithProviders(
    <OnboardDiscoverScreen
      navigation={navigation}
      route={mockRoute('OnboardDiscover')}
    />,
    {
      core,
      platform,
      settings: { 'tailnet.enabled': options.tailnetEnabled ?? true },
      localDirectory: options.localDirectory ?? { peers: [gpuTrain, nasHome] },
    },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...result, core, platform, navigation };
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

it('lists what this phone knows without registering it anywhere', async () => {
  const { core } = await renderScreen();
  expect(await screen.findByText('找到了 6 台电脑')).toBeOnTheScreen();
  expect(
    core.calls.filter(
      call =>
        call.method.startsWith('hub.') ||
        call.method === 'identity.register',
    ),
  ).toEqual([]);
  expect(screen.queryByText('登记这台设备失败')).toBeNull();
  expect(screen.queryByText(/Hub/)).toBeNull();
  expect(screen.queryByText(/安装链接/)).toBeNull();
});

it('lists only what the LAN knows while Tailscale is switched off', async () => {
  const { core } = await renderScreen({
    tailnetEnabled: false,
    localDirectory: { manual: [lanPc], peers: [gxl, gpuTrain, nasHome] },
  });
  expect(await screen.findByText('Meeting PC')).toBeOnTheScreen();
  // No tailnet computers or Tailscale SSH servers: the tailnet is not asked.
  expect(screen.queryByText('可以 SSH')).toBeNull();
  expect(screen.queryByTestId('discover-native-tailnet:home-mac')).toBeNull();
  expect(
    screen.getByText('同一 Wi-Fi 或公司内网里的电脑，填 IP 或名字就行'),
  ).toBeOnTheScreen();
  expect(screen.queryAllByText(/Tailscale|tailnet/)).toHaveLength(0);
  expect(core.calls.map(call => call.method)).not.toContain('tailnet.status');
});

it('shows paired computers and SSH servers and routes their actions', async () => {
  const view = await renderScreen();
  expect(await screen.findByText('找到了 6 台电脑')).toBeOnTheScreen();
  expect(screen.getByText('可以直接连接')).toBeOnTheScreen();
  expect(screen.getByText('4 台')).toBeOnTheScreen();
  expect(screen.getByText('可以 SSH')).toBeOnTheScreen();
  expect(screen.getByText('不用装东西 · 2 台')).toBeOnTheScreen();
  expect(screen.getAllByText('已配对')).toHaveLength(4);
  expect(screen.getByTestId(`discover-remote-${STUDIO_HOST_ID}`)).toBeTruthy();
  // Every computer here is already paired; there is nothing to pair from.
  expect(screen.queryByText('配对')).toBeNull();

  // 「设置」 builds a server entry for this machine, not the phone's key list.
  await fireEvent.press(screen.getByLabelText('设置 gpu-train-02'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('AddServer', {
    prefill: { name: 'gpu-train-02', host: '100.64.0.32' },
  });
  expect(view.navigation.navigate).not.toHaveBeenCalledWith('SshKeys');
  await fireEvent.press(screen.getByText('按地址添加'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('AddByAddress');
});

it('lists system-sharing computers first and keeps the Host install last', async () => {
  const view = await renderScreen({
    localDirectory: {
      manual: [lanPc],
      peers: [gxl, sleepingPc, gpuTrain, nasHome],
    },
  });

  expect(await screen.findByText('找到了 9 台电脑')).toBeOnTheScreen();
  expect(screen.getByText('7 台')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '打开了屏幕共享（Mac）或远程桌面（Windows）的电脑可以直接连，不用装任何东西。',
    ),
  ).toBeOnTheScreen();
  const rows = screen
    .getAllByTestId(/^discover-(native|remote)-/)
    .map(row => row.props.testID as string);
  expect(rows.slice(0, 3)).toEqual([
    'discover-native-tailnet:home-mac',
    'discover-native-tailnet:win-241',
    `discover-native-manual:${lanPc.id}`,
  ]);
  expect(rows.slice(3)).toContain(`discover-remote-${STUDIO_HOST_ID}`);
  expect(rows).toHaveLength(7);
  expect(screen.getAllByText('系统共享')).toHaveLength(3);
  expect(screen.getByText('macOS · 100.64.0.30')).toBeOnTheScreen();
  expect(screen.getByText('Windows · 离线')).toBeOnTheScreen();
  expect(screen.getByText('Windows · 192.168.31.88')).toBeOnTheScreen();

  const tree = JSON.stringify(screen.toJSON());
  const direct = tree.indexOf('可以直接连接');
  const install = tree.indexOf('其他方式：安装 Remote Host');
  expect(direct).toBeGreaterThanOrEqual(0);
  expect(tree.indexOf('可以 SSH')).toBeGreaterThan(direct);
  expect(install).toBeGreaterThan(tree.indexOf('按地址添加'));
  expect(screen.queryByText('添加电脑')).toBeNull();

  await fireEvent.press(screen.getByText('其他方式：安装 Remote Host'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('PairAdd');

  await fireEvent.press(screen.getByTestId('discover-native-tailnet:home-mac'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('NativeSheet', {
    entryKey: 'tailnet:home-mac',
  });
});

it('retries when the paired computers cannot be read', async () => {
  const core = createFakeCore();
  core.failNext('hosts.paired', new Error('disk'));
  await renderScreen({ core });
  expect(await screen.findByText('读取电脑列表失败')).toBeOnTheScreen();
  // The failure replaces the spinner instead of sitting under it.
  expect(screen.queryByText('正在读取电脑列表…')).toBeNull();

  await fireEvent.press(screen.getByText('重试'));
  expect(await screen.findByText('找到了 6 台电脑')).toBeOnTheScreen();
  expect(screen.queryByText('读取电脑列表失败')).toBeNull();
});

it('continues to OnboardProtect without a second button that does the same', async () => {
  const view = await renderScreen();
  expect(screen.queryByText('跳过')).toBeNull();
  await fireEvent.press(screen.getByText('继续'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('OnboardProtect');
  expect(view.navigation.navigate).toHaveBeenCalledTimes(1);
});

it('shows the empty state with Add computer first and still offers the address flow', async () => {
  const view = await renderScreen({ localDirectory: { computers: [] } });
  expect(
    await screen.findByRole('header', { name: '还没有电脑' }),
  ).toBeOnTheScreen();
  expect(
    screen.getByText(
      '打开电脑自带的屏幕共享（Mac：系统设置 › 通用 › 共享）或远程桌面（Windows：设置 › 系统 › 远程桌面），就能直接连，不用装任何东西。',
    ),
  ).toBeOnTheScreen();
  expect(screen.queryByText('可以直接连接')).toBeNull();
  const labels = screen
    .getAllByRole('button')
    .map(button => button.props.accessibilityLabel as string);
  expect(labels.indexOf('添加电脑')).toBeGreaterThanOrEqual(0);
  expect(labels.indexOf('添加电脑')).toBeLessThan(
    labels.indexOf('其他方式：安装 Remote Host'),
  );

  await fireEvent.press(screen.getByText('添加电脑'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('AddComputer');
  await fireEvent.press(screen.getByText('按地址添加'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('AddByAddress');
  await fireEvent.press(screen.getByText('其他方式：安装 Remote Host'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('PairAdd');
});

it('keeps the SSH group in the empty computer state', async () => {
  await renderScreen({ localDirectory: { computers: [], peers: [gpuTrain] } });

  expect(
    await screen.findByRole('header', { name: '还没有电脑' }),
  ).toBeOnTheScreen();
  expect(screen.getByText('可以 SSH')).toBeOnTheScreen();
  expect(screen.getByText('添加电脑')).toBeOnTheScreen();
});
