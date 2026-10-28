import { act, cleanup, renderHook } from '@testing-library/react-native';
import { focusManager } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import {
  NavigationContext,
  NavigationRouteContext,
} from '@react-navigation/native';
import type { ContextType, ReactNode } from 'react';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { pairedHosts, tailnetRunning } from '../../native/rscore/fixtures';
import type {
  Directory,
  DiscoverOutcome,
  TailnetPeer,
  TailscaleDevice,
} from '../../native/rscore/types';
import { leaveHubUrl } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import { MemoryKv } from '../storage/kv';
import type { KvStore } from '../storage/kv';
import { setSettingsStorage } from '../settings/store';
import {
  PROBE_TTL_MS,
  PROBE_WAIT_MS,
  TAILNET_PROBE_KEY,
} from '../tailscale/tailnetProbe';
import { loadDirectory, useDirectory } from './directoryQuery';
import {
  LOCAL_REACH_KEY,
  REACH_INTERVAL_MS,
  REACH_TIMEOUT_MS,
} from './localReach';
import { qk } from './queryClient';

const STUDIO = pairedHosts[0]?.hostId ?? '';
const OFFICE = pairedHosts[1]?.hostId ?? '';

const clients: QueryClient[] = [];
type ProviderOptions = NonNullable<Parameters<typeof renderWithProviders>[1]>;

type Navigation = ContextType<typeof NavigationContext>;
type FakeNavigation = {
  isFocused(): boolean;
  addListener(type: string, cb: () => void): () => void;
};

function navigationOn(focused: { value: boolean }) {
  const listeners = new Set<() => void>();
  const navigation: FakeNavigation = {
    isFocused: () => focused.value,
    addListener: (_type, cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
  return {
    navigation,
    set(next: boolean) {
      focused.value = next;
      for (const listener of [...listeners]) {
        listener();
      }
    },
  };
}

/** Most of these tests describe the tailnet, which needs 外网访问 on. */
function tailnetOn<K extends KvStore>(kv: K): K {
  kv.setJSON('settings.tailnet.enabled', true);
  return kv;
}

async function renderDirectory(
  opts: ProviderOptions & { route?: string; navigation?: FakeNavigation } = {},
) {
  const { route, navigation, ...providerOpts } = opts;
  const providers = await renderWithProviders(<></>, {
    ...providerOpts,
    settings: { 'tailnet.enabled': true, ...providerOpts.settings },
  });
  clients.push(providers.queryClient);
  await providers.unmount();
  const Outer = providers.wrapper;
  function Wrapper({ children }: { children: ReactNode }) {
    if (route === undefined || navigation === undefined) {
      return <Outer>{children}</Outer>;
    }
    return (
      <Outer>
        <NavigationContext.Provider value={navigation as unknown as Navigation}>
          <NavigationRouteContext.Provider
            value={{ key: `${route}-1`, name: route }}
          >
            {children}
          </NavigationRouteContext.Provider>
        </NavigationContext.Provider>
      </Outer>
    );
  }
  const rendered = await renderHook(useDirectory, { wrapper: Wrapper });
  return {
    core: providers.core,
    kv: providers.kv,
    settingsStore: providers.settingsStore,
    queryClient: providers.queryClient,
    ...rendered,
  };
}

function calls(core: FakeCore, method: string): unknown[][] {
  return core.calls.filter(call => call.method === method).map(c => c.args);
}

/** Nothing reaches a Hub API, whatever MMKV still holds. */
function hubCalls(core: FakeCore): string[] {
  return core.calls
    .map(call => call.method)
    .filter(method => method.startsWith('hub.'));
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

function peer(overrides: Partial<TailnetPeer>): TailnetPeer {
  return {
    hostname: 'node',
    dnsName: 'node.ts.example.com.',
    ips: [],
    os: 'macOS',
    online: true,
    tags: [],
    direct: true,
    curAddr: '',
    relay: '',
    peerRelay: '',
    lastHandshakeUs: 0,
    rxBytes: 0,
    txBytes: 0,
    ...overrides,
  };
}

beforeEach(() => {
  jest.useFakeTimers();
  focusManager.setFocused(true);
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  focusManager.setFocused(undefined);
  jest.useRealTimers();
});

describe('the local directory', () => {
  it('builds the directory from paired Hosts and never calls the Hub', async () => {
    const kv = new MemoryKv();
    kv.setJSON(LOCAL_REACH_KEY, { [STUDIO]: { online: true, at: 5 } });
    const { core, result } = await renderDirectory({ kv });
    await advance(10);

    const computers = result.current.directory?.computers ?? [];
    expect(computers.map(c => c.hostId)).toEqual(
      pairedHosts.map(host => host.hostId),
    );
    expect(computers.find(c => c.hostId === STUDIO)?.online).toBe(true);
    expect(computers.find(c => c.hostId === OFFICE)?.online).toBe(false);
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();

    await advance(3 * REACH_INTERVAL_MS);
    expect(hubCalls(core)).toEqual([]);
    expect(calls(core, 'hosts.paired').length).toBeGreaterThan(0);
    expect(kv.getString('cache.directory')).toBeUndefined();
  });

  it('ignores a Hub address an older version left in MMKV', async () => {
    const kv = leaveHubUrl(new MemoryKv());
    kv.setJSON(LOCAL_REACH_KEY, { [OFFICE]: { online: true, at: 5 } });
    const { core, result } = await renderDirectory({ kv });
    await advance(10);

    const computers = result.current.directory?.computers ?? [];
    expect(computers.map(c => c.hostId)).toEqual(
      pairedHosts.map(host => host.hostId),
    );
    expect(computers.find(c => c.hostId === OFFICE)?.online).toBe(true);
    expect(result.current.error).toBeNull();

    await advance(3 * REACH_INTERVAL_MS);
    expect(hubCalls(core)).toEqual([]);
    expect(kv.getString('cache.directory')).toBeUndefined();
  });

  it('never asks for tailnet status without the in-app tailnet', async () => {
    const core = createFakeCore();
    const { result } = await renderDirectory({ core });
    await advance();
    expect(result.current.directory?.computers).toHaveLength(
      pairedHosts.length,
    );
    expect(calls(core, 'tailnet.status')).toEqual([]);
  });

  it('adds tailnet nodes only while the in-app tailnet runs', async () => {
    const core = createFakeCore({ tailnetAvailable: true });
    const { result } = await renderDirectory({ core });
    await advance();
    expect(calls(core, 'tailnet.status').length).toBeGreaterThan(0);
    expect(result.current.directory?.others).toEqual([]);
    expect(result.current.directory?.computers[0]?.node).toBe('');

    core.setTailnet({
      status: {
        ...tailnetRunning,
        peers: [
          ...tailnetRunning.peers,
          peer({ hostname: 'alice-mac', ips: ['100.64.0.40'] }),
          peer({
            hostname: 'gpu-01',
            ips: ['100.64.0.41'],
            os: 'linux',
            tags: ['tag:server'],
          }),
        ],
      },
    });
    await act(() => {
      result.current.refetch().catch(() => undefined);
    });
    await advance(REACH_TIMEOUT_MS + 10);
    expect(result.current.directory?.others.map(o => o.node)).toEqual([
      'alice-mac',
    ]);
    expect(result.current.directory?.servers.map(s => s.node)).toEqual([
      'gpu-01',
    ]);
    expect(
      result.current.directory?.computers.find(c => c.hostId === STUDIO)?.node,
    ).toBe('studio-mac');
  });

  it('drops a forgotten Host once the directory is invalidated', async () => {
    const { core, result, queryClient } = await renderDirectory();
    await advance();
    expect(result.current.directory?.computers).toHaveLength(
      pairedHosts.length,
    );

    await act(() => core.hosts.forget(STUDIO));
    await act(() => queryClient.invalidateQueries({ queryKey: qk.directory }));
    await advance(10);
    expect(
      result.current.directory?.computers.map(c => c.hostId),
    ).not.toContain(STUDIO);
  });

  it('probes only while the Home tab is focused', async () => {
    const focused = { value: true };
    const home = navigationOn(focused);
    const { core } = await renderDirectory({
      route: 'Home',
      navigation: home.navigation,
    });
    await advance(10);
    expect(calls(core, 'diag.probe').map(args => args[0])).toEqual(
      [...pairedHosts.map(host => host.hostId)].sort(),
    );

    await act(() => home.set(false));
    const before = calls(core, 'diag.probe').length;
    await advance(3 * REACH_INTERVAL_MS);
    expect(calls(core, 'diag.probe')).toHaveLength(before);
  });

  it.each([
    ['another screen', 'Session'],
    ['no navigator', undefined],
  ])('does not probe from %s', async (_label, route) => {
    const { core } = await renderDirectory({
      route,
      navigation: navigationOn({ value: true }).navigation,
    });
    await advance(3 * REACH_INTERVAL_MS);
    expect(calls(core, 'diag.probe')).toHaveLength(0);
  });
});

describe('with 外网访问 (Tailscale) switched off', () => {
  it('reads only paired Hosts: no tailnet status, Tailscale API or probes', async () => {
    const core = createFakeCore({ tailnetAvailable: true });
    core.setTailnet({ available: true, status: tailnetRunning });
    const { result } = await renderDirectory({
      core,
      settings: { 'tailnet.enabled': false },
    });
    await advance(10);

    expect(result.current.directory?.computers.map(c => c.hostId)).toEqual(
      pairedHosts.map(host => host.hostId),
    );
    expect(result.current.directory?.others).toEqual([]);
    expect(result.current.directory?.servers).toEqual([]);
    expect(calls(core, 'tailnet.available')).toEqual([]);
    expect(calls(core, 'tailnet.status')).toEqual([]);
    expect(calls(core, 'tailscaleApi.status')).toEqual([]);
  });

  it('keeps loadDirectory off the tailnet too', async () => {
    const kv = new MemoryKv();
    setSettingsStorage(kv);
    const core = createFakeCore();
    core.setTailnet({ available: true, status: tailnetRunning });
    const directory = await loadDirectory(core, kv);
    expect(directory.others).toEqual([]);
    expect(calls(core, 'tailnet.status')).toEqual([]);
    expect(calls(core, 'tailscaleApi.status')).toEqual([]);
  });
});

describe('loadDirectory', () => {
  it('reads paired Hosts despite a Hub address left in MMKV', async () => {
    setSettingsStorage(tailnetOn(leaveHubUrl(new MemoryKv())));
    const core = createFakeCore();
    const directory = await loadDirectory(core);
    expect(directory.computers.map(c => c.hostId)).toEqual(
      pairedHosts.map(host => host.hostId),
    );
    expect(calls(core, 'hosts.paired')).toHaveLength(1);
    expect(hubCalls(core)).toEqual([]);
  });

  it('combines paired Hosts, cached reach and tailnet', async () => {
    const kv = new MemoryKv();
    kv.setJSON(LOCAL_REACH_KEY, { [OFFICE]: { online: true, at: 5 } });
    setSettingsStorage(tailnetOn(kv));
    const core = createFakeCore();
    core.setTailnet({ available: true, status: tailnetRunning });

    const directory = await loadDirectory(core, kv);
    expect(hubCalls(core)).toEqual([]);
    expect(directory.computers.map(c => c.hostId)).toEqual(
      pairedHosts.map(host => host.hostId),
    );
    expect(directory.computers.find(c => c.hostId === OFFICE)).toMatchObject({
      online: true,
      node: 'office-win',
      os: 'windows',
    });

    const withoutKv = await loadDirectory(core);
    expect(withoutKv.computers.every(c => !c.online)).toBe(true);
  });

  it('marks Host addresses inside a registered network as intranet', async () => {
    const company = {
      id: 'net-1',
      name: '公司网络',
      cidrs: ['10.30.0.0/16'],
      vpnPools: ['10.99.0.0/16'],
    };
    const kv = new MemoryKv();
    setSettingsStorage(tailnetOn(kv));
    const core = createFakeCore();
    const before = await loadDirectory(core, kv);
    expect(
      before.computers.find(c => c.hostId === OFFICE)?.addresses,
    ).toContainEqual({ kind: 'lan', addr: '10.30.12.48', port: 7400 });

    kv.setJSON('settings.networks', [company]);
    const after = await loadDirectory(core, kv);
    const office = after.computers.find(c => c.hostId === OFFICE);
    expect(office?.addresses).toEqual([
      { kind: 'tailnet', addr: '100.64.0.12', port: 7400 },
      {
        kind: 'intranet',
        addr: '10.30.12.48',
        port: 7400,
        cidr: '10.30.0.0/16',
      },
    ]);
    // A home LAN outside every registered network stays LAN.
    const studio = after.computers.find(c => c.hostId === STUDIO);
    expect(studio?.addresses).toContainEqual({
      kind: 'lan',
      addr: '192.168.31.20',
      port: 7400,
    });

    const { result } = await renderDirectory({
      core,
      settings: { networks: [company] },
    });
    await advance();
    expect(
      result.current.directory?.computers.find(c => c.hostId === OFFICE)
        ?.addresses[1],
    ).toMatchObject({ kind: 'intranet', cidr: '10.30.0.0/16' });
  });
});

describe('Tailscale API devices', () => {
  function device(overrides: Partial<TailscaleDevice>): TailscaleDevice {
    return {
      id: 'n1',
      name: 'node',
      dnsName: 'node.tail1234.ts.net',
      os: 'linux',
      addresses: [],
      online: true,
      lastSeenAt: null,
      tags: [],
      ...overrides,
    };
  }

  const devices = [
    device({ id: 'n1', name: 'aliyun-bj', addresses: ['100.64.0.60'] }),
    device({
      id: 'n2',
      name: 'alice-mac',
      os: 'macos',
      addresses: ['100.64.0.61'],
    }),
    device({
      id: 'n3',
      name: 'bob-pc',
      os: 'windows',
      addresses: ['100.64.0.62'],
    }),
  ];

  function configured() {
    const core = createFakeCore();
    core.setTailscale({ configured: true, devices });
    return core;
  }

  it('adds the token devices to the directory', async () => {
    const core = configured();
    const { result } = await renderDirectory({ core });
    await advance(10);

    expect(result.current.directory?.servers.map(s => s.node)).toEqual([
      'aliyun-bj',
    ]);
    expect(result.current.directory?.others.map(o => o.node)).toEqual([
      'alice-mac',
      'bob-pc',
    ]);
    expect(result.current.directory?.computers).toHaveLength(
      pairedHosts.length,
    );
    const listed = calls(core, 'tailscaleApi.devices');
    expect(listed.length).toBeGreaterThan(0);
    expect(listed.every(args => args[0] === false)).toBe(true);
  });

  it('keeps the paired Hosts when listing the devices fails', async () => {
    const core = configured();
    core.failNext(
      'tailscaleApi.devices',
      new RsCoreError('http', 'tailscale api unauthorized', 401),
    );
    const { result } = await renderDirectory({ core });
    await advance(10);

    expect(calls(core, 'tailscaleApi.devices')).toHaveLength(1);
    expect(result.current.error).toBeNull();
    expect(result.current.directory?.computers.map(c => c.hostId)).toEqual(
      pairedHosts.map(host => host.hostId),
    );
    expect(result.current.directory?.servers).toEqual([]);
    expect(result.current.directory?.others).toEqual([]);
  });

  it('does not list devices before a token is saved', async () => {
    const core = createFakeCore();
    core.setTailscale({ devices });
    const { result } = await renderDirectory({ core });
    await advance(10);

    expect(calls(core, 'tailscaleApi.status').length).toBeGreaterThan(0);
    expect(calls(core, 'tailscaleApi.devices')).toHaveLength(0);
    expect(result.current.directory?.others).toEqual([]);
  });

  it('lists the token devices despite a Hub address left in MMKV', async () => {
    const core = configured();
    const { result } = await renderDirectory({
      core,
      kv: leaveHubUrl(new MemoryKv()),
    });
    await advance(10);

    expect(result.current.directory?.servers.map(s => s.node)).toEqual([
      'aliyun-bj',
    ]);
    expect(result.current.directory?.others.map(o => o.node)).toEqual([
      'alice-mac',
      'bob-pc',
    ]);
    expect(hubCalls(core)).toEqual([]);
  });

  it('includes the token devices in loadDirectory', async () => {
    const kv = leaveHubUrl(new MemoryKv());
    setSettingsStorage(tailnetOn(kv));
    const core = configured();
    const directory = await loadDirectory(core, kv);
    expect(directory.servers.map(s => s.node)).toEqual(['aliyun-bj']);
    expect(directory.others.map(o => o.node)).toEqual(['alice-mac', 'bob-pc']);
    expect(hubCalls(core)).toEqual([]);
  });
});

describe('Headscale devices', () => {
  const MINUTE_MS = 60_000;

  function node(overrides: Partial<TailscaleDevice>): TailscaleDevice {
    return {
      id: '1',
      name: 'node',
      dnsName: '',
      os: 'unknown',
      addresses: [],
      online: true,
      lastSeenAt: null,
      tags: [],
      ...overrides,
    };
  }

  const nodes = [
    node({ id: '1', name: 'studio', addresses: ['100.64.0.81', 'fd7a::81'] }),
    node({ id: '2', name: 'office-pc', addresses: ['100.64.0.82'] }),
    node({ id: '3', name: 'gpu-box', addresses: ['100.64.0.83'] }),
    node({ id: '4', name: 'away', addresses: ['100.64.0.84'], online: false }),
  ];

  /** `open` lists "addr:port"; other probes time out after `delayMs`. */
  function headscale(open: string[], delayMs = 0) {
    const core = createFakeCore();
    core.setTailscale({
      configured: true,
      controlUrl: 'https://hs.example.com',
      devices: nodes,
    });
    const probe = jest.fn(
      async (addr: string, port: number, _kind: unknown, ms: number) => {
        if (delayMs > 0) {
          await new Promise(resolve => setTimeout(resolve, delayMs));
        }
        const outcome: DiscoverOutcome = open.includes(`${addr}:${port}`)
          ? 'open'
          : 'timeout';
        return { addr, port, outcome, ms };
      },
    );
    if (core.discover === undefined) {
      throw new Error('fake discovery missing');
    }
    core.discover.probe = probe;
    return { core, probe };
  }

  const PORTS = [
    '100.64.0.81:5900',
    '100.64.0.82:3389',
    '100.64.0.83:22',
    '100.64.0.84:5900',
  ];

  /** The probe query starts after the inputs render, one act() later. */
  async function settle() {
    for (let step = 0; step < 3; step += 1) {
      await advance(10);
    }
  }

  function kinds(directory: Directory | undefined) {
    return {
      others: directory?.others.map(other => `${other.node}:${other.os}`),
      servers: directory?.servers.map(server => `${server.node}:${server.os}`),
    };
  }

  it('lists probed devices as a Mac, a Windows PC and an SSH server', async () => {
    const { core, probe } = headscale(PORTS);
    const { result } = await renderDirectory({ core });
    await settle();

    expect(kinds(result.current.directory)).toEqual({
      others: ['studio:macos', 'office-pc:windows'],
      servers: ['gpu-box:linux'],
    });
    // Three ports for each online device; the offline one is never probed.
    expect(probe).toHaveBeenCalledTimes(9);
    expect(probe.mock.calls.some(call => call[0] === '100.64.0.84')).toBe(
      false,
    );
  });

  it('probes again only after 10 minutes or on pull to refresh', async () => {
    const { core, probe } = headscale(PORTS);
    const { result } = await renderDirectory({ core });
    await settle();
    expect(probe).toHaveBeenCalledTimes(9);

    await advance(PROBE_TTL_MS - MINUTE_MS);
    expect(probe).toHaveBeenCalledTimes(9);

    // Pull to refresh (the Home screen's onRefresh calls refetch).
    await act(() => {
      result.current.refetch().catch(() => undefined);
    });
    await advance(REACH_TIMEOUT_MS + 10);
    expect(probe).toHaveBeenCalledTimes(18);

    await advance(PROBE_TTL_MS - 2 * MINUTE_MS);
    expect(probe).toHaveBeenCalledTimes(18);
    await advance(3 * MINUTE_MS);
    expect(probe).toHaveBeenCalledTimes(27);
    expect(kinds(result.current.directory).others).toEqual([
      'studio:macos',
      'office-pc:windows',
    ]);
  });

  it('shows the saved classes while a probe round takes over 2 s', async () => {
    const kv = new MemoryKv();
    kv.setJSON(TAILNET_PROBE_KEY, {
      '1': { addr: '100.64.0.81', at: 0, vnc: true, rdp: false, ssh: false },
    });
    const { core } = headscale(['100.64.0.81:3389'], 5_000);
    const { result } = await renderDirectory({ core, kv });
    await settle();

    expect(result.current.isLoading).toBe(false);
    expect(kinds(result.current.directory)).toEqual({
      others: ['studio:macos'],
      servers: [],
    });

    await advance(5_000);
    await settle();
    expect(kinds(result.current.directory)).toEqual({
      others: ['studio:windows'],
      servers: [],
    });
  });

  it('lets loadDirectory wait at most 2 s for probes', async () => {
    const kv = new MemoryKv();
    setSettingsStorage(tailnetOn(kv));
    kv.setJSON(TAILNET_PROBE_KEY, {
      '1': { addr: '100.64.0.81', at: 0, vnc: true, rdp: false, ssh: false },
    });
    const { core, probe } = headscale(['100.64.0.81:3389'], 5_000);

    let directory: Directory | undefined;
    loadDirectory(core, kv).then(value => {
      directory = value;
    });
    await advance(PROBE_WAIT_MS - 10);
    expect(directory).toBeUndefined();
    await advance(20);
    expect(kinds(directory)).toEqual({
      others: ['studio:macos'],
      servers: [],
    });
    expect(probe).toHaveBeenCalledTimes(9);

    // Without kv (no saved classes) nothing is probed.
    probe.mockClear();
    const plain = await loadDirectory(core);
    expect(probe).not.toHaveBeenCalled();
    expect(plain.others).toEqual([]);
  });

  it('probes despite a Hub address left in MMKV', async () => {
    const { core, probe } = headscale(PORTS);
    const { result } = await renderDirectory({
      core,
      kv: leaveHubUrl(new MemoryKv()),
    });
    await settle();
    expect(probe).toHaveBeenCalledTimes(9);
    expect(kinds(result.current.directory).servers).toEqual(['gpu-box:linux']);
    expect(hubCalls(core)).toEqual([]);
  });
});
