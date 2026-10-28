import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
} from '@testing-library/react-native';
import type { TabScreenProps } from '../../../app/navigation/routes';
import { installFakeAdapters } from '../../../features/adapters/testing/fakeAdapters';
import { createFakeCore } from '../../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../../native/rscore/fixtures';
import type { SshServer, TailscaleDevice } from '../../../native/rscore/types';
import { HISTORY_KEY } from '../../../features/sessions/history';
import { MemoryKv } from '../../../features/storage/kv';
import { FakePlatform } from '../../../native/platform/fakePlatform';
import { mockWindowDimensions } from '../../../test/dimensions';
import { tailnetPeer } from '../../../test/localDirectory';
import type { LocalDirectorySeed } from '../../../test/localDirectory';
import { mockNavigation } from '../../../test/navigation';
import { renderWithProviders } from '../../../test/render';
import HomeScreen from '..';

jest.setTimeout(30_000);

// useConnect navigates through the root ref; route it to the screen's mock.
let mockRootNavigate: jest.Mock | undefined;
jest.mock('../../../app/navigation/navigationRef', () => ({
  navigationRef: {
    isReady: () => true,
    navigate: (...args: unknown[]) => mockRootNavigate?.(...args),
  },
}));

const STUDIO_ID = 'h7v2m9q3k8x4tb1a';
const OFFICE_ID = 'w3k8p2x7m4q9tb6c';
const HOME_PC_ID = 'd2n6c4v8b1x5za7e';
const MACBOOK_ID = 'm5q1z8r2k7v3xa9d';

/** A Mac on the tailnet without Host; the local directory names it by node. */
const officeMac = tailnetPeer({
  hostname: 'office-mac',
  ips: ['100.64.0.40'],
  os: 'macOS',
});

async function renderHome(options?: {
  core?: ReturnType<typeof createFakeCore>;
  kv?: MemoryKv;
  platform?: FakePlatform;
  settings?: NonNullable<Parameters<typeof renderWithProviders>[1]>['settings'];
  /** Defaults to the paired fixture computers on a running tailnet. */
  localDirectory?: LocalDirectorySeed | null;
}) {
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation'];
  mockRootNavigate = navigation.navigate as jest.Mock;
  const rendered = await renderWithProviders(
    <HomeScreen
      navigation={navigation}
      route={{ key: 'Home-test', name: 'Home' }}
    />,
    {
      core: options?.core,
      kv: options?.kv,
      platform: options?.platform,
      settings: { 'tailnet.enabled': true, ...options?.settings },
      localDirectory:
        options?.localDirectory === null
          ? undefined
          : options?.localDirectory ?? { peers: [] },
    },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...rendered, navigation };
}

let restoreDimensions: (() => void) | undefined;

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockWindowDimensions({ width: 1194, height: 834 });
});

afterEach(async () => {
  await cleanup();
  restoreDimensions?.();
  mockRootNavigate = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('renders the sidebar with four paired computers, servers and the live Studio selection', async () => {
  await renderHome();

  expect(screen.getByTestId('home-wide')).toBeOnTheScreen();
  expect(screen.getByText('设备')).toBeOnTheScreen();
  // Only a Hub listed the unpaired 实验室 Mac.
  expect(screen.getAllByTestId(/^wide-computer-/)).toHaveLength(4);
  expect(screen.getAllByTestId(/^server-/)).toHaveLength(4);
  expect(screen.getAllByText('Studio Mac mini').length).toBeGreaterThan(0);
  expect(
    screen.getByText('studio-mac · 100.64.0.7 · macOS 26.1'),
  ).toBeOnTheScreen();
  expect(screen.getAllByText('回到会话').length).toBeGreaterThan(0);
});

it('selects a sidebar computer without navigating, then opens the preview', async () => {
  const { navigation } = await renderHome();

  await fireEvent.press(screen.getByTestId(`wide-computer-${OFFICE_ID}`));
  expect(screen.getAllByText('工位 Windows').length).toBeGreaterThan(0);
  expect(navigation.navigate).not.toHaveBeenCalled();

  await fireEvent.press(screen.getByLabelText('控制 工位 Windows 的显示器 1'));
  expect(navigation.navigate).toHaveBeenCalledWith('Session', {
    hostId: OFFICE_ID,
  });
});

it('opens the device sheet on long press and the terminal for the selected computer', async () => {
  const { navigation } = await renderHome();

  await fireEvent(
    screen.getByTestId(`wide-computer-${OFFICE_ID}`),
    'longPress',
  );
  expect(navigation.navigate).toHaveBeenCalledWith('DeviceSheet', {
    hostId: OFFICE_ID,
  });

  await fireEvent.press(screen.getByTestId(`wide-computer-${OFFICE_ID}`));
  await fireEvent.press(screen.getByLabelText('终端'));
  expect(navigation.navigate).toHaveBeenCalledWith('Terminal', {
    serverId: 'office-win',
  });

  // H20: the disabled file button is gone until files can be sent from here.
  expect(screen.queryByLabelText('传文件，后续版本提供')).toBeNull();
  expect(screen.queryByText('传文件')).toBeNull();
});

it('shows the Studio displays, host permissions and last connection duration', async () => {
  const kv = new MemoryKv({
    [HISTORY_KEY]: JSON.stringify([
      {
        id: 'history-studio',
        kind: 'remote',
        targetId: STUDIO_ID,
        name: 'Studio Mac mini',
        startedAtUs: FIXTURE_NOW - 42 * 60_000_000,
        endedAtUs: FIXTURE_NOW - 1_000_000,
        durationS: 2520,
      },
    ]),
  });
  await renderHome({ kv });

  expect(screen.getByTestId('wide-info-displays')).toHaveTextContent('2 台', {
    exact: false,
  });
  expect(screen.getByTestId('wide-info-displays')).toHaveTextContent(
    '2560×1600 · 2160×3840',
    { exact: false },
  );
  expect(screen.getByTestId('wide-info-host')).toHaveTextContent(
    '0.3.1 · 权限齐全',
    { exact: false },
  );
  expect(screen.getByTestId('wide-info-last')).toHaveTextContent('42 分钟', {
    exact: false,
  });
});

it('filters by name, node or address like the phone, and selects the office desktop', async () => {
  await renderHome();

  // H30: the iPad no longer matches the OS, so only office-win matches "win";
  // 家里台式机 (Windows, node home-pc) drops out like it does on the phone.
  await fireEvent.changeText(screen.getByLabelText('搜索设备'), 'win');
  expect(screen.getAllByTestId(/^wide-computer-/)).toHaveLength(1);
  expect(screen.getByTestId(`wide-computer-${OFFICE_ID}`)).toBeOnTheScreen();
  expect(screen.queryByTestId(`wide-computer-${HOME_PC_ID}`)).toBeNull();
  expect(screen.getAllByText('工位 Windows').length).toBeGreaterThan(0);

  await fireEvent.changeText(screen.getByLabelText('搜索设备'), '100.64.0.7');
  expect(screen.getAllByTestId(/^wide-computer-/)).toHaveLength(1);
  expect(screen.getByTestId(`wide-computer-${STUDIO_ID}`)).toBeOnTheScreen();
});

it('renders the narrow home without the sidebar at phone width', async () => {
  restoreDimensions?.();
  restoreDimensions = mockWindowDimensions({ width: 390, height: 844 });
  await renderHome();

  expect(screen.queryByTestId('home-wide')).toBeNull();
  expect(screen.queryByTestId('wide-sidebar')).toBeNull();
  expect(screen.getByTestId(`card-${STUDIO_ID}`)).toBeOnTheScreen();
});

it('offers no wake for the selected offline computer, which needs a relay', async () => {
  const { navigation } = await renderHome();

  await fireEvent.press(screen.getByTestId(`wide-computer-${MACBOOK_ID}`));

  expect(screen.queryByLabelText('唤醒 小明的 MacBook Pro')).toBeNull();
  expect(screen.queryByText('唤醒')).toBeNull();
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('keeps the control action for the selected online computer', async () => {
  const core = createFakeCore({ sessions: [] });
  await renderHome({ core });

  await fireEvent.press(screen.getByTestId(`wide-computer-${STUDIO_ID}`));

  expect(screen.queryByLabelText('唤醒 Studio Mac mini')).toBeNull();
  expect(
    screen.getByLabelText('控制 Studio Mac mini 的显示器 1'),
  ).toBeOnTheScreen();
  expect(screen.getByText('控制')).toBeOnTheScreen();
});

it('lists a tailnet Mac without Host, shows its pane and connects natively', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const { navigation } = await renderHome({
    core,
    localDirectory: { peers: [officeMac] },
  });

  const row = screen.getByTestId('wide-computer-tailnet:office-mac');
  expect(row).toHaveProp(
    'accessibilityLabel',
    'office-mac，Mac · 屏幕共享，在线，系统共享',
  );
  await fireEvent.press(row);
  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(screen.getByTestId('wide-native-pane')).toBeOnTheScreen();
  expect(screen.getByText('100.64.0.40')).toBeOnTheScreen();

  await fireEvent.press(screen.getByTestId('wide-native-connect'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('AdapterSession', {
    target: { protocol: 'vnc', addr: '100.64.0.40' },
    name: 'office-mac',
    addrKind: 'tailnet',
    context: 'native',
  });

  await fireEvent.press(screen.getByLabelText('更多操作'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('NativeSheet', {
    entryKey: 'tailnet:office-mac',
  });
});

it('shows the external-app guide from the native pane when the app is missing', async () => {
  const core = createFakeCore();
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const { navigation } = await renderHome({
    core,
    platform,
    localDirectory: { peers: [officeMac] },
  });

  await fireEvent.press(screen.getByTestId('wide-computer-tailnet:office-mac'));
  await fireEvent.press(screen.getByTestId('wide-native-connect'));

  const guide = await screen.findByTestId('fallback-guide-card');
  expect(within(guide).getByText('需要一个屏幕共享 App')).toBeOnTheScreen();
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('shows the native empty state and both add entries without computers', async () => {
  const core = createFakeCore({ sshServers: [] });
  const { navigation } = await renderHome({
    core,
    localDirectory: { computers: [] },
  });

  expect(screen.getAllByText('还没有电脑').length).toBeGreaterThan(0);
  await fireEvent.press(screen.getByText('添加电脑'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');
  await fireEvent.press(screen.getByText('其他方式：安装 Remote Host'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('PairAdd');
});

it('lists the Tailscale Linux suggestion in the sidebar', async () => {
  const dbServer: SshServer = {
    serverId: 'manual-db',
    name: 'db',
    host: '100.64.0.61',
    port: 22,
    user: 'alice',
    jump: null,
    auth: 'deviceKey',
    tailscaleSsh: null,
    source: 'manual',
  };
  const core = createFakeCore({
    pairedHosts: [],
    sshServers: [dbServer],
    sessions: [],
  });
  core.setTailscale({
    configured: true,
    devices: ['aliyun-bj', 'db-01'].map(
      (name, index): TailscaleDevice => ({
        id: name,
        name,
        dnsName: `${name}.tail1234.ts.net`,
        os: 'linux',
        addresses: [`100.64.0.6${index}`],
        online: true,
        lastSeenAt: null,
        tags: [],
      }),
    ),
  });
  const { navigation } = await renderHome({ core, localDirectory: null });

  const sidebar = screen.getByTestId('wide-sidebar');
  const row = await within(sidebar).findByTestId('server-suggest:aliyun-bj');
  expect(within(sidebar).getAllByTestId(/^server-/)).toHaveLength(2);
  expect(within(sidebar).getByTestId('server-manual-db')).toBeOnTheScreen();
  expect(
    within(row).getByText('100.64.0.60 · 来自 Tailscale · 点按添加'),
  ).toBeOnTheScreen();

  await fireEvent.press(row);
  expect(navigation.navigate).toHaveBeenCalledTimes(1);
  expect(navigation.navigate).toHaveBeenCalledWith('AddServer', {
    prefill: { name: 'aliyun-bj', host: '100.64.0.60', port: 22 },
  });
});
