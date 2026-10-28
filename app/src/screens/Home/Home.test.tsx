import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import {
  AccessibilityInfo,
  BackHandler,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { installFakeAdapters } from '../../features/adapters/testing/fakeAdapters';
import { MemoryKv } from '../../features/storage/kv';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  computers,
  netCompanyVpn,
  netCompanyVpnEmbedded,
  netNoTailnet,
  pairedHosts,
  routeEmbeddedNeedsLogin,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import type {
  Computer,
  SshServer,
  TailscaleDevice,
} from '../../native/rscore/types';
import type { ManualComputer } from '../../features/servers/manual';
import { resetTailnetSwitchStore } from '../../features/tailnet/switchStore';
import { leaveHubUrl, tailnetPeer } from '../../test/localDirectory';
import type { LocalDirectorySeed } from '../../test/localDirectory';
import { mockNavigation } from '../../test/navigation';
import {
  PHONE_WINDOW,
  mockPhoneDimensions,
  mockWindowDimensions,
} from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import HomeScreen from '.';

// useConnect navigates through the root ref; route it to the screen's mock.
let mockRootNavigate: jest.Mock | undefined;
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    isReady: () => true,
    navigate: (...args: unknown[]) => mockRootNavigate?.(...args),
  },
}));

let restoreDimensions: (() => void) | undefined;

const OFFICE_ID = 'w3k8p2x7m4q9tb6c';
const HOME_PC_ID = 'd2n6c4v8b1x5za7e';

// A Mac on the tailnet without Host; the local directory names it by node.
const officeMac = tailnetPeer({
  hostname: 'office-mac',
  ips: ['100.64.0.40'],
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

/** An SSH server saved on this phone. */
const manualDb: SshServer = {
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

const serversValue = {
  version: 1,
  value: { v: 1, entries: [labPc], tailscaleSsh: {} },
};

/** One Host computer, one tailnet Mac without Host and one manual PC. */
function mixedCore(): ReturnType<typeof createFakeCore> {
  return createFakeCore({ sshServers: [], sessions: [] });
}

const MIXED: LocalDirectorySeed = {
  computers: computers.filter(computer => computer.hostId === OFFICE_ID),
  peers: [officeMac],
  manual: [labPc],
};

/** The paired fixture computers, changed by `mutate`. */
function computersWith(mutate: (items: Computer[]) => Computer[]): Computer[] {
  return mutate(computers.map(computer => ({ ...computer })));
}

async function renderHome(options?: {
  core?: ReturnType<typeof createFakeCore>;
  kv?: MemoryKv;
  platform?: FakePlatform;
  settings?: NonNullable<Parameters<typeof renderWithProviders>[1]>['settings'];
  /** Defaults to the paired fixture computers; null keeps the core as is. */
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
      // Most of these tests describe Tailscale on Home, so 外网访问 is on.
      settings: { 'tailnet.enabled': true, ...options?.settings },
      localDirectory:
        options?.localDirectory === null
          ? undefined
          : options?.localDirectory ?? {},
    },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...rendered, navigation };
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockPhoneDimensions();
});

afterEach(async () => {
  await cleanup();
  restoreDimensions?.();
  restoreDimensions = undefined;
  mockRootNavigate = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('renders the title, sections, four paired cards, servers and live session', async () => {
  await renderHome();

  expect(screen.getByText('设备')).toBeOnTheScreen();
  expect(screen.getByText('电脑')).toBeOnTheScreen();
  expect(screen.getByText('服务器')).toBeOnTheScreen();
  // Only a Hub knew computers this phone never paired with.
  expect(
    screen.getAllByTestId(/^card-/).map(card => card.props.testID),
  ).toEqual(
    expect.arrayContaining([
      'card-h7v2m9q3k8x4tb1a',
      'card-w3k8p2x7m4q9tb6c',
      'card-m5q1z8r2k7v3xa9d',
      'card-d2n6c4v8b1x5za7e',
    ]),
  );
  expect(screen.getAllByTestId(/^card-/)).toHaveLength(4);
  expect(screen.getByText('回到会话')).toBeOnTheScreen();
  expect(screen.getByTestId('live-clock')).toHaveTextContent('42:18');
});

describe('with 外网访问 (Tailscale) switched off', () => {
  it('names Tailscale nowhere, even with the company VPN pushing it out', async () => {
    const core = createFakeCore({
      sshServers: [],
      sessions: [],
      network: netCompanyVpn,
      tailnetAvailable: true,
    });
    await renderHome({
      core,
      kv: new MemoryKv(),
      settings: {
        'tailnet.enabled': false,
        networks: [
          {
            id: 'n1',
            name: '公司网络',
            cidrs: ['10.30.0.0/16'],
            vpnPools: ['10.99.0.0/16'],
          },
        ],
      },
      localDirectory: MIXED,
    });

    expect(screen.getByText('设备')).toBeOnTheScreen();
    // The tailnet Mac without Host is not listed: the tailnet is never asked.
    expect(screen.queryByText('office-mac')).toBeNull();
    expect(screen.getByText('Lab PC')).toBeOnTheScreen();
    expect(screen.queryAllByText(/Tailscale|tailnet|Headscale/)).toHaveLength(
      0,
    );
    expect(core.calls.map(call => call.method)).not.toContain(
      'tailnet.status',
    );
    expect(core.calls.map(call => call.method)).not.toContain(
      'tailscaleApi.status',
    );
  });
});

it('opens live and new sessions, and the version error page', async () => {
  const { navigation } = await renderHome({
    localDirectory: {
      computers: computersWith(items =>
        items.map(item =>
          item.name === '家里台式机'
            ? { ...item, online: true, hostRunning: true, hostVersion: '0.2.4' }
            : item,
        ),
      ),
    },
  });

  await fireEvent.press(screen.getByTestId('card-h7v2m9q3k8x4tb1a'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('Session', {
    hostId: 'h7v2m9q3k8x4tb1a',
    sessionId: 's-live-1',
  });

  await fireEvent.press(screen.getByTestId('card-w3k8p2x7m4q9tb6c'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('Session', {
    hostId: 'w3k8p2x7m4q9tb6c',
  });

  await fireEvent.press(screen.getByTestId('card-d2n6c4v8b1x5za7e'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('SessionEnded', {
    hostId: 'd2n6c4v8b1x5za7e',
    hostName: '家里台式机',
    code: 170,
  });
});

it('long-presses a card for details', async () => {
  const { navigation } = await renderHome();

  await fireEvent(screen.getByTestId('card-w3k8p2x7m4q9tb6c'), 'longPress');

  expect(navigation.navigate).toHaveBeenCalledWith('DeviceSheet', {
    hostId: 'w3k8p2x7m4q9tb6c',
  });
});

it('navigates to pairing, settings, terminal and the live session', async () => {
  const { navigation } = await renderHome();

  await fireEvent.press(screen.getByLabelText('添加设备'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');

  await fireEvent.press(screen.getByLabelText('账号与设置'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('MainTabs', {
    screen: 'Settings',
  });

  await fireEvent.press(
    screen.getByLabelText('prod-api-01，Ubuntu 24.04 · Tailscale SSH'),
  );
  expect(navigation.navigate).toHaveBeenLastCalledWith('Terminal', {
    serverId: 'prod-api-01',
  });

  await fireEvent.press(screen.getByTestId('live-session-bar'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('Session', {
    hostId: 'h7v2m9q3k8x4tb1a',
    sessionId: 's-live-1',
  });
});

it('filters cards and clears the search', async () => {
  await renderHome();
  const search = screen.getByLabelText('搜索设备');

  await fireEvent.changeText(search, 'studio');
  expect(screen.getAllByTestId(/^card-/)).toHaveLength(1);

  await fireEvent.changeText(search, 'windows');
  expect(screen.getAllByTestId(/^card-/)).toHaveLength(1);
  expect(screen.getByText('工位 Windows')).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('清除搜索'));
  expect(screen.getAllByTestId(/^card-/)).toHaveLength(4);
});

it('ticks the live-session clock every second', async () => {
  await renderHome();

  await act(async () => {
    await jest.advanceTimersByTimeAsync(1000);
  });
  expect(screen.getByTestId('live-clock')).toHaveTextContent('42:19');
});

it('offers an empty-directory action', async () => {
  const core = createFakeCore({ sshServers: [] });
  const { navigation } = await renderHome({
    core,
    localDirectory: { computers: [] },
  });

  expect(screen.getByText('还没有电脑')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '打开电脑自带的屏幕共享（Mac：系统设置 › 通用 › 共享）或远程桌面（Windows：设置 › 系统 › 远程桌面），就能直接连，不用装任何东西。',
    ),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('添加电脑'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');

  await fireEvent.press(screen.getByText('其他方式：安装 Remote Host'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('PairAdd');
});

it('exposes accessible labels for cards and icon buttons', async () => {
  await renderHome();

  expect(screen.getByLabelText(/^Studio Mac mini，/)).toBeOnTheScreen();
  expect(screen.getByLabelText('添加设备')).toBeOnTheScreen();
  expect(screen.getByLabelText('账号与设置')).toBeOnTheScreen();
});

it('renders the wide two-column home at 1024 px', async () => {
  restoreDimensions?.();
  restoreDimensions = mockWindowDimensions({ width: 1024, height: 768 });
  const { navigation } = await renderHome();

  expect(screen.getByTestId('home-wide')).toBeOnTheScreen();
  expect(screen.getByTestId('wide-sidebar')).toBeOnTheScreen();
  expect(screen.getByText('设备')).toBeOnTheScreen();
  expect(screen.getAllByTestId(/^wide-computer-/).length).toBeGreaterThan(0);

  await fireEvent.press(screen.getByLabelText('添加设备'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');
});

it('switches the card grid to one column at large dynamic type', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(1.4);
  await renderHome();

  const studio = screen.getByTestId('card-h7v2m9q3k8x4tb1a');
  const office = screen.getByTestId('card-w3k8p2x7m4q9tb6c');
  const studioRow = studio.parent;
  const officeRow = office.parent;
  expect(StyleSheet.flatten(studio.parent?.props.style)).toMatchObject({
    width: PHONE_WINDOW.width - 40,
  });
  expect(studioRow?.parent).not.toBe(officeRow?.parent);
});

it('says the device list could not be read and retries it', async () => {
  const core = createFakeCore();
  core.failNext('hosts.paired', new Error('offline'));
  await renderHome({ core });

  expect(screen.getByText('读不到设备列表')).toBeOnTheScreen();
  expect(screen.queryByText('连不上 Hub')).toBeNull();
  await fireEvent.press(screen.getByText('重试'));
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });

  expect(
    await screen.findByTestId('card-h7v2m9q3k8x4tb1a'),
  ).toBeOnTheScreen();
  expect(screen.queryByText('读不到设备列表')).toBeNull();
});

it('shows the offline Tailscale banner and opens the platform URL', async () => {
  const core = createFakeCore({ network: netNoTailnet });
  const { platform } = await renderHome({ core });

  expect(screen.getByText('Tailscale 没有连接')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('打开 Tailscale'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(platform.opened[0]).toBe('tailscale://');
});

it('saves the iOS embedded address and announces network changes only in the header', async () => {
  resetTailnetSwitchStore();
  jest.replaceProperty(Platform, 'OS', 'ios');
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const core = createFakeCore({
    tailnetAvailable: true,
    network: netCompanyVpn,
  });
  const { settingsStore } = await renderHome({
    core,
    settings: {
      networks: [
        {
          id: 'company',
          name: '公司网络',
          cidrs: ['10.30.0.0/16'],
          vpnPools: ['10.99.0.0/16'],
        },
      ],
    },
  });

  expect(screen.queryByText('为什么')).toBeNull();
  await fireEvent.press(screen.getByText('改用 App 内连接'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByTestId('tailnet-control-url-sheet')).toBeOnTheScreen();
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByText('继续'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.queryByTestId('tailnet-control-url-sheet')).toBeNull();
  expect(settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
  expect(settingsStore.get('tailnet.mode')).toBe('embedded');

  await act(async () => {
    core.emit({
      type: 'network.changed',
      status: { ...netCompanyVpn, tailnet: routeEmbeddedNeedsLogin },
    });
  });
  expect(screen.getByText('App 内连接需要登录')).toBeOnTheScreen();
  expect(announce).not.toHaveBeenCalled();

  await act(async () => {
    core.emit({ type: 'network.changed', status: netCompanyVpnEmbedded });
  });
  expect(screen.getByText('Tailscale 和公司 VPN 同时可用')).toBeOnTheScreen();
  // Studio and the office PC; the unpaired Lab Mac was listed only by a Hub.
  expect(announce.mock.calls).toEqual([['Tailscale · App 内 · 2 台在线']]);
});

describe('computers without Host (V10)', () => {
  it('renders Host, tailnet and manual cards; only the last two carry the badge', async () => {
    await renderHome({ core: mixedCore(), localDirectory: MIXED });

    expect(
      screen.getAllByTestId(/^card-/).map(card => card.props.testID),
    ).toEqual([
      `card-${OFFICE_ID}`,
      'card-tailnet:office-mac',
      'card-manual:lab-pc',
    ]);
    expect(
      screen.getAllByTestId(/^native-badge-/).map(badge => badge.props.testID),
    ).toEqual(['native-badge-tailnet:office-mac', 'native-badge-manual:lab-pc']);
    expect(screen.getByTestId('native-badge-tailnet:office-mac')).toHaveTextContent(
      '系统共享',
    );
    expect(screen.getByText('Mac · 屏幕共享')).toBeOnTheScreen();
    expect(screen.getByText('Windows · 远程桌面')).toBeOnTheScreen();
    expect(screen.getByText('Tailscale · 2 台在线')).toBeOnTheScreen();
  });

  it('shows 检测中 while a manual computer has not answered a probe', async () => {
    const core = mixedCore();
    jest
      .spyOn(core.discover!, 'probe')
      .mockReturnValue(new Promise(() => undefined));
    await renderHome({ core, localDirectory: MIXED });

    expect(
      screen.getByLabelText('Lab PC，Windows · 远程桌面，在线状态未知，系统共享'),
    ).toBeOnTheScreen();
    expect(screen.getByText('检测中…')).toBeOnTheScreen();
    expect(screen.queryByText('离线')).toBeNull();
  });

  it('marks a manual computer offline once its probe times out', async () => {
    const core = mixedCore();
    await renderHome({ core, localDirectory: MIXED });

    expect(
      await screen.findByLabelText('Lab PC，Windows · 远程桌面，离线，系统共享'),
    ).toBeOnTheScreen();
    expect(core.calls).toContainEqual({
      method: 'discover.probe',
      args: ['192.168.31.50', 3389, 'rdp', 800],
    });
  });

  it('opens a tailnet Mac in the built-in adapter and long-presses into its sheet', async () => {
    const core = mixedCore();
    installFakeAdapters(core);
    const { navigation } = await renderHome({ core, localDirectory: MIXED });

    await fireEvent.press(screen.getByTestId('card-tailnet:office-mac'));
    expect(navigation.navigate).toHaveBeenLastCalledWith('AdapterSession', {
      target: { protocol: 'vnc', addr: '100.64.0.40' },
      name: 'office-mac',
      addrKind: 'tailnet',
      context: 'native',
    });

    await fireEvent(screen.getByTestId('card-tailnet:office-mac'), 'longPress');
    expect(navigation.navigate).toHaveBeenLastCalledWith('NativeSheet', {
      entryKey: 'tailnet:office-mac',
    });

    await fireEvent(screen.getByTestId('card-manual:lab-pc'), 'longPress');
    expect(navigation.navigate).toHaveBeenLastCalledWith('NativeSheet', {
      entryKey: 'manual:lab-pc',
    });

    await fireEvent(screen.getByTestId(`card-${OFFICE_ID}`), 'longPress');
    expect(navigation.navigate).toHaveBeenLastCalledWith('DeviceSheet', {
      hostId: OFFICE_ID,
    });
  });

  it('shows the external-app guide when a computer without Host has no app', async () => {
    const platform = new FakePlatform();
    platform.openUrlResult = false;
    const { navigation } = await renderHome({
      core: mixedCore(),
      platform,
      localDirectory: MIXED,
    });
    expect(screen.queryByTestId('fallback-guide-card')).toBeNull();

    await fireEvent.press(screen.getByTestId('card-tailnet:office-mac'));

    const guide = await screen.findByTestId('fallback-guide-card');
    expect(within(guide).getByText('需要一个屏幕共享 App')).toBeOnTheScreen();
    expect(navigation.navigate).not.toHaveBeenCalled();
    await fireEvent.press(within(guide).getByRole('button', { name: '取消' }));
    expect(screen.queryByTestId('fallback-guide-card')).toBeNull();
  });

  it('opens a manual PC with its own address in the built-in adapter', async () => {
    const core = mixedCore();
    installFakeAdapters(core);
    const { navigation } = await renderHome({ core, localDirectory: MIXED });

    await fireEvent.press(screen.getByTestId('card-manual:lab-pc'));
    expect(navigation.navigate).toHaveBeenLastCalledWith('AdapterSession', {
      target: { protocol: 'rdp', addr: '192.168.31.50' },
      name: 'Lab PC',
      addrKind: 'manual',
      context: 'native',
    });
  });

  /** The home PC is online with a Host too old for this app. */
  function outdatedHomePc(): Computer[] {
    return computersWith(items =>
      items.map(item =>
        item.hostId === HOME_PC_ID
          ? { ...item, online: true, hostRunning: true, hostVersion: '0.2.4' }
          : item,
      ),
    );
  }

  it('probes Remote Desktop for an outdated Host instead of the version page', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    const probe = jest.spyOn(core.discover!, 'probe').mockResolvedValue({
      addr: '100.64.0.15',
      port: 3389,
      outcome: 'open',
      ms: 4,
    });
    const { navigation } = await renderHome({
      core,
      localDirectory: { computers: outdatedHomePc() },
    });

    await fireEvent.press(screen.getByTestId(`card-${HOME_PC_ID}`));
    await waitFor(() =>
      expect(navigation.navigate).toHaveBeenLastCalledWith('AdapterSession', {
        target: { protocol: 'rdp', hostId: HOME_PC_ID, addr: '100.64.0.15' },
        name: '家里台式机',
        addrKind: 'tailnet',
        context: 'native',
      }),
    );
    expect(probe).toHaveBeenCalledWith('100.64.0.15', 3389, 'rdp', 1500);
    expect(navigation.navigate).not.toHaveBeenCalledWith(
      'SessionEnded',
      expect.anything(),
    );
  });

  it('keeps the version page when that computer is set to use only Host', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    const probe = jest.spyOn(core.discover!, 'probe');
    const { navigation } = await renderHome({
      core,
      settings: { 'hosts.prefs': { [HOME_PC_ID]: { connectVia: 'host' } } },
      localDirectory: { computers: outdatedHomePc() },
    });

    await fireEvent.press(screen.getByTestId(`card-${HOME_PC_ID}`));
    expect(navigation.navigate).toHaveBeenLastCalledWith('SessionEnded', {
      hostId: HOME_PC_ID,
      hostName: '家里台式机',
      code: 170,
    });
    expect(probe).not.toHaveBeenCalled();
  });

  it('lists paired, tailnet and manual computers and servers, and refreshes them', async () => {
    const core = createFakeCore({
      pairedHosts: pairedHosts.filter(host => host.hostId === OFFICE_ID),
      tailnetAvailable: true,
      tailnet: {
        ...tailnetRunning,
        peers: [
          // Paired over this address, so it is the Host card, not a tailnet one.
          ...tailnetRunning.peers.filter(peer => peer.hostname === 'office-win'),
          {
            ...tailnetRunning.peers[0],
            hostname: 'office-mac',
            dnsName: 'office-mac.ts.example.com.',
            ips: ['100.64.0.40'],
            os: 'macOS',
          },
        ],
      },
      sshServers: [manualDb],
      sessions: [],
    });
    const kv = new MemoryKv({
      'sync.local.servers': JSON.stringify(serversValue),
    });
    await renderHome({ core, kv, localDirectory: null });

    expect(
      await screen.findByTestId('card-tailnet:office-mac'),
    ).toBeOnTheScreen();
    expect(screen.getByTestId(`card-${OFFICE_ID}`)).toBeOnTheScreen();
    expect(screen.getByTestId('card-manual:lab-pc')).toBeOnTheScreen();
    expect(screen.queryByTestId('card-tailnet:office-win')).toBeNull();
    expect(
      screen.getByTestId('native-badge-tailnet:office-mac'),
    ).toBeOnTheScreen();
    expect(screen.getByTestId('native-badge-manual:lab-pc')).toBeOnTheScreen();
    expect(screen.getByText('Mac · 屏幕共享')).toBeOnTheScreen();
    expect(screen.getByTestId('server-manual-db')).toBeOnTheScreen();
    expect(screen.queryByTestId('card-h7v2m9q3k8x4tb1a')).toBeNull();

    // Refreshing (pull down, or the header's refresh action) reads the
    // paired Hosts again.
    core.setFixtures({
      pairedHosts: pairedHosts.filter(
        host => host.hostId === OFFICE_ID || host.hostId === 'h7v2m9q3k8x4tb1a',
      ),
    });
    const header = screen
      .getAllByLabelText('电脑')
      .find(element =>
        element.props.accessibilityActions?.some(
          (action: { name: string }) => action.name === 'refresh',
        ),
      );
    await fireEvent(header!, 'accessibilityAction', {
      nativeEvent: { actionName: 'refresh' },
    });
    await act(async () => {
      await jest.advanceTimersByTimeAsync(1);
    });
    expect(
      await screen.findByTestId('card-h7v2m9q3k8x4tb1a'),
    ).toBeOnTheScreen();
    expect(screen.getByTestId('card-manual:lab-pc')).toBeOnTheScreen();
    expect(screen.getByTestId('card-tailnet:office-mac')).toBeOnTheScreen();

    expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(
      false,
    );
    expect(screen.queryAllByText(/Hub/)).toEqual([]);
  });

  it('works with a Hub address an older version left in MMKV', async () => {
    const core = createFakeCore({ sshServers: [manualDb], sessions: [] });
    const kv = new MemoryKv();
    leaveHubUrl(kv);
    await renderHome({
      core,
      kv,
      localDirectory: MIXED,
    });

    expect(screen.getByTestId(`card-${OFFICE_ID}`)).toBeOnTheScreen();
    expect(screen.getByTestId('card-tailnet:office-mac')).toBeOnTheScreen();
    expect(screen.getByTestId('card-manual:lab-pc')).toBeOnTheScreen();
    expect(screen.getByTestId('server-manual-db')).toBeOnTheScreen();
    expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(
      false,
    );
    expect(screen.queryAllByText(/Hub/)).toEqual([]);
  });

  it('shows the new empty state without a Hub and without computers', async () => {
    const core = createFakeCore({
      pairedHosts: [],
      sshServers: [],
      sessions: [],
    });
    const { navigation } = await renderHome({ core, localDirectory: null });

    expect(await screen.findByText('还没有电脑')).toBeOnTheScreen();
    expect(screen.queryByText('连不上 Hub')).toBeNull();
    await fireEvent.press(screen.getByText('添加电脑'));
    expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');
  });
});

describe('Tailscale Linux suggestions', () => {
  const dbServer = manualDb;

  function linux(
    name: string,
    addr: string,
    overrides: Partial<TailscaleDevice> = {},
  ): TailscaleDevice {
    return {
      id: name,
      name,
      dnsName: `${name}.tail1234.ts.net`,
      os: 'linux',
      addresses: [addr],
      online: true,
      lastSeenAt: null,
      tags: [],
      ...overrides,
    };
  }

  function tailscaleCore(
    sshServers: SshServer[],
    devices = [
      linux('aliyun-bj', '100.64.0.60'),
      linux('db-01', '100.64.0.61'),
    ],
  ) {
    const core = createFakeCore({
      pairedHosts: [],
      sshServers,
      sessions: [],
    });
    core.setTailscale({ configured: true, devices });
    return core;
  }

  it('suggests the one Linux machine no server covers and opens Add Server prefilled', async () => {
    const { navigation } = await renderHome({
      core: tailscaleCore([dbServer]),
      localDirectory: null,
    });

    const row = await screen.findByTestId('server-suggest:aliyun-bj');
    expect(screen.getAllByTestId(/^server-suggest:/)).toHaveLength(1);
    expect(screen.getByTestId('server-manual-db')).toBeOnTheScreen();
    expect(
      within(row).getByText('100.64.0.60 · 来自 Tailscale · 点按添加'),
    ).toBeOnTheScreen();
    expect(within(row).getByText('在线')).toBeOnTheScreen();
    expect(row).toHaveProp(
      'accessibilityLabel',
      'aliyun-bj，100.64.0.60，在线，来自 Tailscale，点按添加为 SSH 服务器',
    );

    await fireEvent.press(row);
    expect(navigation.navigate).toHaveBeenCalledTimes(1);
    expect(navigation.navigate).toHaveBeenCalledWith('AddServer', {
      prefill: { name: 'aliyun-bj', host: '100.64.0.60', port: 22 },
    });
  });

  it('shows an offline suggestion instead of the empty page', async () => {
    await renderHome({
      core: tailscaleCore(
        [],
        [linux('aliyun-bj', '100.64.0.60', { online: false })],
      ),
      localDirectory: null,
    });

    const row = await screen.findByTestId('server-suggest:aliyun-bj');
    expect(within(row).getByText('离线')).toBeOnTheScreen();
    expect(row).toHaveProp(
      'accessibilityLabel',
      'aliyun-bj，100.64.0.60，离线，来自 Tailscale，点按添加为 SSH 服务器',
    );
    expect(screen.getByText('服务器')).toBeOnTheScreen();
  });
});

describe('interaction round 2', () => {
  const STUDIO_ID = 'h7v2m9q3k8x4tb1a';

  it('shows a spinner on the card it is connecting and holds the others', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    // Remote Desktop's reachability probe never answers.
    const probe = jest
      .spyOn(core.discover!, 'probe')
      .mockReturnValue(new Promise(() => undefined));
    const platform = new FakePlatform();
    const { navigation } = await renderHome({ core, platform });

    await fireEvent.press(screen.getByTestId(`card-${OFFICE_ID}`));
    expect(
      await screen.findByTestId(`card-${OFFICE_ID}-busy`),
    ).toBeOnTheScreen();
    expect(
      screen.getByTestId(`card-${OFFICE_ID}`).props.accessibilityState,
    ).toMatchObject({ busy: true });
    expect(platform.calls).toContainEqual({
      method: 'haptic',
      args: ['light'],
    });

    // The other cards wait; a second tap starts nothing.
    expect(screen.getByTestId(`card-${STUDIO_ID}`)).toBeDisabled();
    await fireEvent.press(screen.getByTestId(`card-${STUDIO_ID}`));
    await fireEvent.press(screen.getByTestId(`card-${OFFICE_ID}`));
    expect(navigation.navigate).not.toHaveBeenCalled();
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it('keeps the sharing guide first and offers Tailscale as a secondary way on an empty list', async () => {
    const core = createFakeCore({
      network: netNoTailnet,
      sshServers: [],
      sessions: [],
    });
    // Only someone who set Tailscale up hears that it is down.
    core.setTailscale({ configured: true });
    const { navigation, platform } = await renderHome({
      core,
      localDirectory: { computers: [] },
    });

    expect(screen.getByTestId('home-empty-tailnet')).toBeOnTheScreen();
    expect(screen.queryByTestId('home-empty')).toBeNull();
    // Without Tailscale the app still works on the local network (App Review 4.2).
    expect(screen.getByText('还没有电脑')).toBeOnTheScreen();
    expect(
      screen.getByText(
        '打开电脑自带的屏幕共享（Mac：系统设置 › 通用 › 共享）或远程桌面（Windows：设置 › 系统 › 远程桌面），就能直接连，不用装任何东西。',
      ),
    ).toBeOnTheScreen();
    expect(
      screen.getByText(
        'Tailscale 没有连接。只有在外面连回电脑时才需要它，同一网络里的电脑可以直接添加。',
      ),
    ).toBeOnTheScreen();
    // H02: the empty state already says it, so there is no banner as well.
    expect(screen.queryByTestId('home-banner-tailnetDown')).toBeNull();

    await fireEvent.press(screen.getByTestId('home-empty-open-tailscale'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(platform.opened[0]).toBe('tailscale://');

    await fireEvent.press(screen.getByTestId('home-empty-add'));
    expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');
  });

  it('does not mention Tailscale to someone who never set it up', async () => {
    const core = createFakeCore({
      network: netNoTailnet,
      sshServers: [],
      sessions: [],
    });
    await renderHome({ core, localDirectory: { computers: [] } });

    expect(screen.getByTestId('home-empty')).toBeOnTheScreen();
    expect(screen.queryByTestId('home-empty-tailnet')).toBeNull();
    expect(screen.queryByTestId('home-banner-tailnetDown')).toBeNull();
    expect(screen.queryByText('Tailscale 没有连接')).toBeNull();
    expect(screen.queryByText('Tailscale 已断开')).toBeNull();
  });

  it('names the live session after the computer list', async () => {
    await renderHome({
      localDirectory: {
        computers: computersWith(items =>
          items.map(item =>
            item.hostId === STUDIO_ID ? { ...item, name: '书房 Mac' } : item,
          ),
        ),
      },
    });

    const bar = screen.getByTestId('live-session-bar');
    expect(within(bar).getByText('书房 Mac')).toBeOnTheScreen();
    expect(within(bar).queryByText('Studio Mac mini')).toBeNull();
  });

  it('clears the search on Android back before leaving', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    type Handler = Parameters<typeof BackHandler.addEventListener>[1];
    const handlers: Handler[] = [];
    jest
      .spyOn(BackHandler, 'addEventListener')
      .mockImplementation((_event, listener) => {
        handlers.push(listener);
        return {
          remove: () => {
            handlers.splice(handlers.indexOf(listener), 1);
          },
        };
      });
    const back = () =>
      [...handlers]
        .reverse()
        .some(handler => handler({ type: 'hardwareBackPress', timeStamp: 0 }));
    await renderHome();

    expect(back()).toBe(false);
    await fireEvent.changeText(screen.getByLabelText('搜索设备'), 'studio');
    expect(screen.getAllByTestId(/^card-/)).toHaveLength(1);

    let handled = false;
    await act(async () => {
      handled = back();
    });
    expect(handled).toBe(true);
    expect(screen.getAllByTestId(/^card-/)).toHaveLength(4);
    expect(back()).toBe(false);
  });

  it('searches names and addresses, not the operating system', async () => {
    await renderHome();
    const search = screen.getByLabelText('搜索设备');

    await fireEvent.changeText(search, '100.64.0.15');
    expect(
      screen.getAllByTestId(/^card-/).map(card => card.props.testID),
    ).toEqual([`card-${HOME_PC_ID}`]);

    await fireEvent.changeText(search, 'macOS');
    expect(screen.queryAllByTestId(/^card-/)).toHaveLength(0);
    expect(screen.getByText('没有匹配的电脑')).toBeOnTheScreen();
  });
});
