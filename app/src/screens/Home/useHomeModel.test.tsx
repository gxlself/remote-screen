import {
  act,
  cleanup,
  renderHook,
  waitFor,
} from '@testing-library/react-native';
import { Platform } from 'react-native';
import { focusManager } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import type { TabScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  computers,
  netCompanyVpn,
  netCompanyVpnEmbedded,
  netNoTailnet,
  routeEmbeddedNeedsLogin,
  tailnetNeedsLogin,
} from '../../native/rscore/fixtures';
import { FakePlatform } from '../../native/platform/fakePlatform';
import type { SshServer, TailscaleDevice } from '../../native/rscore/types';
import type { ManualComputer } from '../../features/servers/manual';
import type { NetworkEntry } from '../../features/settings/schema';
import { resetTailnetSwitchStore } from '../../features/tailnet/switchStore';
import {
  adapterLive,
  resetAdapterLive,
} from '../../features/adapters/liveStore';
import { MemoryKv } from '../../features/storage/kv';
import { leaveHubUrl, tailnetPeer } from '../../test/localDirectory';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { EMBEDDED_BOTH_DISMISSED_KEY, useHomeModel } from './useHomeModel';

const COMPANY_NETWORK: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

type ProviderOptions = Parameters<typeof renderWithProviders>[1];
type HomeNavigation = TabScreenProps<'Home'>['navigation'];

const clients: QueryClient[] = [];

/** Without options, the paired fixture computers on a running tailnet. */
async function renderHome(
  query = '',
  opts: ProviderOptions = { localDirectory: { peers: [] } },
  navigation: HomeNavigation = mockNavigation<'MainTabs'>() as unknown as HomeNavigation,
) {
  // Most of these describe Tailscale on Home, which needs 外网访问 on.
  const providers = await renderWithProviders(<></>, {
    ...opts,
    settings: { 'tailnet.enabled': true, ...opts.settings },
  });
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(
    ({ value }: { value: string }) => useHomeModel(value, navigation),
    { initialProps: { value: query }, wrapper: providers.wrapper },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return {
    ...rendered,
    core: providers.core,
    platform: providers.platform,
    kv: providers.kv,
    settingsStore: providers.settingsStore,
    navigation,
  };
}

function prewarmCalls(core: ReturnType<typeof createFakeCore>) {
  return core.calls.filter(call => call.method === 'sessions.prewarm');
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  focusManager.setFocused(true);
});

afterEach(async () => {
  await cleanup();
  resetAdapterLive();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  focusManager.setFocused(undefined);
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('models the default directory, status, banners, servers and session', async () => {
  const { result, core } = await renderHome();

  // The unpaired 实验室 Mac was listed only by a Hub.
  expect(result.current.cards.map(card => card.name)).toEqual([
    'Studio Mac mini',
    '工位 Windows',
    '小明的 MacBook Pro',
    '家里台式机',
  ]);
  expect(result.current.headerStatus).toEqual({
    tone: 'good',
    text: 'Tailscale · 2 台在线',
  });
  expect(result.current.loadError).toBeNull();
  expect(result.current.banners).toEqual([]);
  expect(result.current.servers).toHaveLength(4);
  expect(result.current.liveSession?.hostId).toBe('h7v2m9q3k8x4tb1a');

  const prewarm = prewarmCalls(core);
  expect(prewarm.length).toBeGreaterThan(0);
  expect((prewarm.at(-1)?.args[0] as string[]).length).toBeLessThanOrEqual(4);
});

it('filters cards with the search query', async () => {
  const { result, rerender } = await renderHome();

  await rerender({ value: 'studio' });
  expect(result.current.cards.map(card => card.name)).toEqual([
    'Studio Mac mini',
  ]);

  await rerender({ value: 'windows' });
  expect(result.current.cards.map(card => card.name)).toEqual(['工位 Windows']);
});

it('shows the VPN conflict banner and tailnet-dependent directory servers', async () => {
  const core = createFakeCore();
  const { result } = await renderHome('', {
    core,
    settings: { networks: [COMPANY_NETWORK] },
  });

  await act(async () => {
    core.emit({ type: 'network.changed', status: netCompanyVpn });
  });

  expect(result.current.banners.map(banner => banner.id)).toContain(
    'vpnConflict',
  );
  expect(result.current.headerStatus).toEqual({
    tone: 'warn',
    text: 'Tailscale 已断开 · 公司 VPN',
  });
  expect(
    result.current.servers.find(server => server.serverId === 'prod-api-01')
      ?.needsTailnet,
  ).toBe(true);
  expect(
    result.current.servers.find(
      server => server.serverId === 'manual-build-srv',
    ),
  ).toMatchObject({
    needsTailnet: false,
    subtitle: '内网 · 经 jump-01',
  });
});

it('offers the Tailscale deep link and falls back to the App Store', async () => {
  const platform = new FakePlatform();
  const core = createFakeCore({ network: netNoTailnet });
  const { result } = await renderHome('', { core, platform });

  expect(result.current.banners[0]?.id).toBe('tailnetDown');
  await act(async () => {
    result.current.banners[0]?.action?.onPress();
    await Promise.resolve();
  });
  expect(platform.opened[0]).toBe('tailscale://');

  platform.openUrlResult = false;
  platform.opened.length = 0;
  await act(async () => {
    result.current.banners[0]?.action?.onPress();
    await Promise.resolve();
  });
  expect(platform.opened).toEqual([
    'tailscale://',
    'https://apps.apple.com/app/tailscale/id1470499037',
  ]);
});

it('uses the Android VPN conflict message', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const core = createFakeCore();
  const { result } = await renderHome('', {
    core,
    settings: { networks: [COMPANY_NETWORK] },
  });

  await act(async () => {
    core.emit({ type: 'network.changed', status: netCompanyVpn });
  });

  expect(result.current.banners).toEqual([
    expect.objectContaining({
      id: 'vpnConflict',
      message: 'Android 同时只能开一个 VPN · 先断开公司 VPN，再打开 Tailscale',
    }),
  ]);
});

it('keeps the iOS VPN conflict message', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const core = createFakeCore();
  const { result } = await renderHome('', {
    core,
    settings: { networks: [COMPANY_NETWORK] },
  });

  await act(async () => {
    core.emit({ type: 'network.changed', status: netCompanyVpn });
  });

  expect(result.current.banners).toEqual([
    expect.objectContaining({
      id: 'vpnConflict',
      message: 'iOS 同时只能开一个系统 VPN · 先断开公司 VPN，再打开 Tailscale',
    }),
  ]);
});

it('reports a directory that could not be read without a banner', async () => {
  const core = createFakeCore();
  const error = new Error('offline');
  core.failNext('hosts.paired', error);

  const { result } = await renderHome('', { core, localDirectory: {} });

  expect(result.current.loadError).toBe(error);
  expect(result.current.banners).toEqual([]);
  expect(result.current.cards).toEqual([]);
  expect(result.current.isEmpty).toBe(false);
});

it('maps the directory node to the terminal route for a computer', async () => {
  const navigation = mockNavigation<'MainTabs'>() as unknown as HomeNavigation;
  const { result } = await renderHome('', undefined, navigation);

  result.current.actions.openTerminal('w3k8p2x7m4q9tb6c');

  expect(navigation.navigate).toHaveBeenCalledWith('Terminal', {
    serverId: 'office-win',
  });
});

describe('embedded tailnet banners', () => {
  beforeEach(() => {
    resetTailnetSwitchStore();
    jest.replaceProperty(Platform, 'OS', 'ios');
  });

  it('offers the iOS switch and requests a missing Headscale address', async () => {
    const { result, settingsStore } = await renderHome('', {
      core: createFakeCore({ tailnetAvailable: true, network: netCompanyVpn }),
      settings: { networks: [COMPANY_NETWORK] },
    });

    expect(result.current.banners[0]).toMatchObject({
      id: 'vpnConflict',
      title: '公司 VPN 已连接，系统 Tailscale 被断开了',
      message: 'iOS 同时只能开一个系统 VPN',
      announce: false,
      action: { label: '改用 App 内连接' },
    });
    expect(result.current.banners[0]?.onUseEmbedded).toBeUndefined();

    await act(async () => {
      result.current.banners[0]?.action?.onPress();
    });
    expect(result.current.banners[0]?.controlUrlSheet).toBeDefined();
    expect(settingsStore.get('tailnet.mode')).toBe('system');

    await act(() => result.current.banners[0]?.controlUrlSheet?.onClose());
    expect(result.current.banners[0]?.controlUrlSheet).toBeUndefined();
    await act(async () => {
      result.current.banners[0]?.action?.onPress();
    });
    await act(() => result.current.banners[0]?.controlUrlSheet?.onDone());
    expect(result.current.banners[0]?.controlUrlSheet).toBeUndefined();
  });

  it('switches with a saved address and shows the simultaneous connection', async () => {
    const core = createFakeCore({
      tailnetAvailable: true,
      network: netCompanyVpn,
    });
    const { result, settingsStore, navigation } = await renderHome('', {
      core,
      settings: {
        networks: [COMPANY_NETWORK],
        'tailnet.controlUrl': 'https://hs.example.com',
      },
      localDirectory: {},
    });

    await act(async () => {
      result.current.banners[0]?.action?.onPress();
    });
    expect(settingsStore.get('tailnet.mode')).toBe('embedded');
    expect(result.current.banners[0]?.controlUrlSheet).toBeUndefined();

    await act(async () => {
      core.emit({ type: 'network.changed', status: netCompanyVpnEmbedded });
    });
    expect(result.current.banners.map(banner => banner.id)).toEqual([
      'embeddedBoth',
    ]);
    expect(result.current.headerStatus).toEqual({
      tone: 'good',
      // Studio and the office PC are the paired computers online.
      text: 'Tailscale · App 内 · 2 台在线',
    });
    expect(result.current.banners[0]).toMatchObject({
      tone: 'good',
      announce: false,
      title: 'Tailscale 和公司 VPN 同时可用',
      // H58: the message no longer shows this phone's tailnet address.
      message: 'App 内连接不占系统 VPN，两边各走各的',
      action: { label: '设置' },
    });
    expect(result.current.banners[0]?.secondary).toBeUndefined();
    expect(result.current.servers.every(server => !server.needsTailnet)).toBe(
      true,
    );
    result.current.banners[0]?.action?.onPress();
    expect(navigation.navigate).toHaveBeenCalledWith('MainTabs', {
      screen: 'Settings',
    });
  });

  it('hides the simultaneous-connection banner for good once dismissed', async () => {
    const core = createFakeCore({
      tailnetAvailable: true,
      network: netCompanyVpn,
    });
    const { result, kv, unmount } = await renderHome('', {
      core,
      settings: {
        networks: [COMPANY_NETWORK],
        'tailnet.controlUrl': 'https://hs.example.com',
      },
      localDirectory: {},
    });
    await act(async () => {
      result.current.banners[0]?.action?.onPress();
    });
    await act(async () => {
      core.emit({ type: 'network.changed', status: netCompanyVpnEmbedded });
    });
    expect(result.current.banners[0]?.id).toBe('embeddedBoth');

    await act(async () => {
      result.current.banners[0]?.onDismiss?.();
    });
    expect(result.current.banners.map(banner => banner.id)).not.toContain(
      'embeddedBoth',
    );
    expect(kv.getString(EMBEDDED_BOTH_DISMISSED_KEY)).toBe('1');
    await unmount();

    // A later launch reads the choice back.
    const again = await renderHome('', {
      core,
      kv,
      settings: { networks: [COMPANY_NETWORK] },
    });
    await act(async () => {
      core.emit({ type: 'network.changed', status: netCompanyVpnEmbedded });
    });
    expect(again.result.current.banners.map(banner => banner.id)).not.toContain(
      'embeddedBoth',
    );
  });

  it('opens the embedded node login URL', async () => {
    const { result, platform } = await renderHome('', {
      core: createFakeCore({
        tailnetAvailable: true,
        network: { ...netCompanyVpn, tailnet: routeEmbeddedNeedsLogin },
        tailnet: tailnetNeedsLogin,
      }),
      settings: { networks: [COMPANY_NETWORK] },
    });

    expect(result.current.banners[0]).toMatchObject({
      id: 'embeddedLogin',
      tone: 'warn',
      announce: false,
      title: 'App 内连接需要登录',
      message: '在浏览器里用 Headscale 账号登录，登录后自动连上',
      action: { label: '去登录' },
    });
    await act(async () => {
      result.current.banners[0]?.action?.onPress();
    });
    expect(platform.opened).toEqual([
      'https://hs.example.com/register/nodekey-test',
    ]);
  });

  it('omits the success banner when this run did not switch modes', async () => {
    const { result } = await renderHome('', {
      core: createFakeCore({
        tailnetAvailable: true,
        network: netCompanyVpnEmbedded,
      }),
      settings: {
        networks: [COMPANY_NETWORK],
        'tailnet.mode': 'embedded',
      },
    });
    expect(result.current.banners).toEqual([]);
    expect(result.current.headerStatus.tone).toBe('good');
  });
});

describe('computers without Host (V10)', () => {
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

  function mixedCore() {
    return createFakeCore({ sshServers: [], sessions: [] });
  }

  /** The sleeping MacBook, a tailnet Mac without Host and a manual PC. */
  const mixed = {
    computers: computers.filter(
      computer => computer.hostId === 'm5q1z8r2k7v3xa9d',
    ),
    peers: [
      tailnetPeer({ hostname: 'office-mac', ips: ['100.64.0.40'] }),
    ],
    manual: [labPc],
  };

  function probes(core: ReturnType<typeof createFakeCore>) {
    return core.calls.filter(call => call.method === 'discover.probe');
  }

  it('lists native computers, counts the online ones and sorts offline last', async () => {
    const { result } = await renderHome('', {
      core: mixedCore(),
      localDirectory: mixed,
    });

    // The manual PC drops behind the sleeping MacBook once its probe times out.
    await waitFor(() =>
      expect(
        result.current.cards.map(card =>
          card.kind === 'native' ? card.key : card.hostId,
        ),
      ).toEqual(['tailnet:office-mac', 'm5q1z8r2k7v3xa9d', 'manual:lab-pc']),
    );
    expect(result.current.headerStatus.text).toBe('Tailscale · 1 台在线');
    expect(result.current.isEmpty).toBe(false);
  });

  it('keeps Tailscale out of a local-only list', async () => {
    const core = createFakeCore({
      network: netNoTailnet,
      sshServers: [],
      sessions: [],
    });
    const { result } = await renderHome('', {
      core,
      localDirectory: { computers: [], manual: [labPc] },
    });

    await waitFor(() => expect(result.current.cards).toHaveLength(1));
    expect(result.current.banners).toEqual([]);
    expect(result.current.emptyReason).toBeUndefined();
    expect(result.current.headerStatus.tone).toBe('unknown');
    expect(result.current.headerStatus.text).not.toContain('Tailscale');
  });

  it('warns once the user added a computer by its tailnet address', async () => {
    const core = createFakeCore({
      network: netNoTailnet,
      sshServers: [],
      sessions: [],
    });
    const { result } = await renderHome('', {
      core,
      localDirectory: {
        computers: [],
        manual: [{ ...labPc, host: '100.64.0.50' }],
      },
    });

    await waitFor(() => expect(result.current.cards).toHaveLength(1));
    expect(result.current.banners.map(banner => banner.id)).toEqual([
      'tailnetDown',
    ]);
    expect(result.current.headerStatus).toEqual({
      tone: 'warn',
      text: 'Tailscale 已断开',
    });
  });

  it('warns once a Tailscale API token is saved', async () => {
    const core = createFakeCore({
      network: netNoTailnet,
      sshServers: [],
      sessions: [],
    });
    core.setTailscale({ configured: true });
    const { result } = await renderHome('', {
      core,
      localDirectory: { computers: [], manual: [labPc] },
    });

    await waitFor(() =>
      expect(result.current.banners.map(banner => banner.id)).toEqual([
        'tailnetDown',
      ]),
    );
  });

  it('finds native computers by name and address', async () => {
    const { result, rerender } = await renderHome('', {
      core: mixedCore(),
      localDirectory: mixed,
    });

    await rerender({ value: '192.168.31' });
    expect(result.current.cards.map(card => card.name)).toEqual(['Lab PC']);

    // The local directory names a tailnet computer after its node.
    await rerender({ value: 'office' });
    expect(result.current.cards.map(card => card.name)).toEqual(['office-mac']);
  });

  it('probes manual computers only while Home is focused', async () => {
    const listeners: Record<string, () => void> = {};
    const navigation =
      mockNavigation<'MainTabs'>() as unknown as HomeNavigation;
    (navigation.addListener as unknown as jest.Mock).mockImplementation(
      (event: string, callback: () => void) => {
        listeners[event] = callback;
        return jest.fn();
      },
    );
    const core = mixedCore();
    await renderHome('', { core, localDirectory: mixed }, navigation);
    expect(probes(core)).toHaveLength(1);

    await act(async () => {
      listeners.blur?.();
    });
    await act(async () => {
      await jest.advanceTimersByTimeAsync(60_000);
    });
    expect(probes(core)).toHaveLength(1);

    await act(async () => {
      listeners.focus?.();
      await jest.advanceTimersByTimeAsync(1);
    });
    expect(probes(core)).toHaveLength(2);
  });

  it('sends Add to the new Add Computer page and keeps pairing as the other way', async () => {
    const navigation =
      mockNavigation<'MainTabs'>() as unknown as HomeNavigation;
    const { result } = await renderHome('', undefined, navigation);

    result.current.actions.add();
    expect(navigation.navigate).toHaveBeenLastCalledWith('AddComputer');
    result.current.actions.installHost();
    expect(navigation.navigate).toHaveBeenLastCalledWith('PairAdd');
  });

  it('is empty without paired Hosts, tailnet or manual computers', async () => {
    const core = createFakeCore({ pairedHosts: [], sshServers: [] });
    const { result } = await renderHome('', { core });

    await waitFor(() => expect(result.current.isEmpty).toBe(true));
    expect(result.current.loadError).toBeNull();
    expect(result.current.banners).toEqual([]);
    expect(result.current.cards).toEqual([]);
    expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(
      false,
    );
  });

  it('ignores a Hub address an older version left in MMKV', async () => {
    const core = createFakeCore({ sshServers: [], sessions: [] });
    const { result } = await renderHome('', {
      core,
      kv: leaveHubUrl(new MemoryKv()),
      localDirectory: mixed,
    });

    await waitFor(() =>
      expect(result.current.cards.map(card => card.name)).toEqual([
        'office-mac',
        '小明的 MacBook Pro',
        'Lab PC',
      ]),
    );
    expect(result.current.loadError).toBeNull();
    expect(result.current.banners).toEqual([]);
    expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(
      false,
    );
  });
});

describe('Tailscale server suggestions', () => {
  function manual(serverId: string, name: string, host: string): SshServer {
    return {
      serverId,
      name,
      host,
      port: 22,
      user: 'alice',
      jump: null,
      auth: 'deviceKey',
      tailscaleSsh: null,
      source: 'manual',
    };
  }

  function linux(name: string, addr: string): TailscaleDevice {
    return {
      id: name,
      name,
      dnsName: `${name}.tail1234.ts.net`,
      os: 'linux',
      addresses: [addr],
      online: true,
      lastSeenAt: null,
      tags: [],
    };
  }

  function tailscaleCore(sshServers: SshServer[], devices: TailscaleDevice[]) {
    const core = createFakeCore({ pairedHosts: [], sshServers, sessions: [] });
    core.setTailscale({ configured: true, devices });
    return core;
  }

  const aliyun = {
    serverId: 'suggest:aliyun-bj',
    name: 'aliyun-bj',
    subtitle: '100.64.0.60 · 来自 Tailscale · 点按添加',
    needsTailnet: false,
    suggestion: { host: '100.64.0.60', online: true },
  };

  it('drops machines a server covers by address, name or id, ignoring case', async () => {
    const core = tailscaleCore(
      [
        manual('manual-web', 'Web-01', '10.30.20.5'),
        manual('CI-Runner', 'ci', '10.30.20.6'),
        manual('manual-db', 'db', '100.64.0.61'),
      ],
      [
        linux('web-01', '100.64.0.70'),
        linux('ci-runner', '100.64.0.71'),
        linux('db-01', '100.64.0.61'),
        linux('aliyun-bj', '100.64.0.60'),
      ],
    );
    const { result, rerender } = await renderHome('', { core });

    await waitFor(() =>
      expect(result.current.servers.map(server => server.serverId)).toEqual([
        'manual-web',
        'CI-Runner',
        'manual-db',
        'suggest:aliyun-bj',
      ]),
    );
    expect(result.current.servers.at(-1)).toEqual(aliyun);

    await rerender({ value: 'aliyun' });
    expect(result.current.servers).toEqual([aliyun]);
    await rerender({ value: 'web' });
    expect(result.current.servers.map(server => server.serverId)).toEqual([
      'manual-web',
    ]);
  });

  it('is not empty with only a suggestion, which opens Add Server prefilled', async () => {
    const navigation =
      mockNavigation<'MainTabs'>() as unknown as HomeNavigation;
    const { result } = await renderHome(
      '',
      {
        core: tailscaleCore([], [linux('aliyun-bj', '100.64.0.60')]),
      },
      navigation,
    );

    await waitFor(() => expect(result.current.servers).toEqual([aliyun]));
    expect(result.current.isEmpty).toBe(false);

    result.current.actions.openServer(aliyun);
    expect(navigation.navigate).toHaveBeenLastCalledWith('AddServer', {
      prefill: { name: 'aliyun-bj', host: '100.64.0.60', port: 22 },
    });
  });
});

describe('folded-away screen sharing in the bar (V33)', () => {
  const target = { protocol: 'rdp' as const, addr: '192.168.1.20' };

  function putAdapter(startedAtUs: number) {
    adapterLive.put({
      sessionId: 'adapter-1',
      params: { target, name: '家里台式机', addrKind: 'manual' },
      name: '家里台式机',
      phase: 'streaming',
      error: null,
      screens: [],
      screenIndex: 0,
      mode: 'trackpad',
      startedAtUs,
      collapsedAtUs: FIXTURE_NOW,
      history: {
        live: null,
        stoppedAtUs: null,
        error: null,
        name: '家里台式机',
        targetId: 'addr:rdp:192.168.1.20',
      },
    });
  }

  it('shows the latest session and returns to it by id', async () => {
    putAdapter(FIXTURE_NOW - 60e6);
    const { result, navigation } = await renderHome();

    expect(result.current.liveSession).toMatchObject({
      id: 'adapter-1',
      hostName: '家里台式机',
      path: null,
      adapter: { phase: 'streaming' },
    });
    // A typed address matches no computer card.
    expect(
      result.current.cards.some(
        card => card.kind !== 'native' && card.hostId === result.current.liveSession?.hostId,
      ),
    ).toBe(false);

    await act(async () => result.current.actions.backToSession());
    expect(navigation.navigate).toHaveBeenCalledWith('AdapterSession', {
      target,
      name: '家里台式机',
      addrKind: 'manual',
      sessionId: 'adapter-1',
    });
  });

  it('keeps the Host session when it started later', async () => {
    // The fixture Host session started 2538 s before FIXTURE_NOW.
    putAdapter(FIXTURE_NOW - 3000e6);
    const { result } = await renderHome();

    expect(result.current.liveSession?.adapter).toBeUndefined();
    expect(result.current.liveSession?.hostId).toBe('h7v2m9q3k8x4tb1a');
  });

  it('goes back to the Host session once the adapter one is gone', async () => {
    putAdapter(FIXTURE_NOW - 60e6);
    const { result } = await renderHome();
    expect(result.current.liveSession?.adapter).toBeDefined();

    await act(async () => {
      adapterLive.take('adapter-1');
    });
    expect(result.current.liveSession?.hostId).toBe('h7v2m9q3k8x4tb1a');
  });
});
