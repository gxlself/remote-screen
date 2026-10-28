import { computers } from '../../native/rscore/fixtures';
import type { ComputerEntry } from './types';
import { decideRoute } from './decideRoute';

type HostEntry = Extract<ComputerEntry, { kind: 'host' }>;

function hostEntry(
  overrides: Partial<(typeof computers)[number]> = {},
): HostEntry {
  return {
    kind: 'host',
    key: 'host:h1',
    computer: {
      ...computers[0],
      ...overrides,
      addresses: overrides.addresses ?? computers[0].addresses,
    },
  };
}

it('routes host entries through the host when explicitly preferred', () => {
  expect(
    decideRoute({
      entry: hostEntry(),
      adaptersAvailable: true,
      connectPrefer: 'host',
      discoverAvailable: true,
    }),
  ).toEqual({ kind: 'session', hostId: computers[0].hostId });
});

it('requests a native probe and routes an open result to AdapterSession', () => {
  const entry = hostEntry();
  const probe = decideRoute({
    entry,
    adaptersAvailable: true,
    connectPrefer: 'native',
    discoverAvailable: true,
  });
  expect(probe).toMatchObject({
    kind: 'probe',
    addr: '100.64.0.7',
    port: 5900,
    protocol: 'vnc',
    addrKind: 'tailnet',
  });

  expect(
    decideRoute({
      entry,
      adaptersAvailable: true,
      connectPrefer: 'native',
      discoverAvailable: true,
      probe: 'open',
    }),
  ).toMatchObject({
    kind: 'adapter',
    target: {
      protocol: 'vnc',
      hostId: computers[0].hostId,
      addr: '100.64.0.7',
    },
    addrKind: 'tailnet',
    context: 'native',
  });
});

it('distinguishes host fallback, sharing guide, and timeout routes', () => {
  const online = hostEntry();
  expect(
    decideRoute({
      entry: online,
      adaptersAvailable: true,
      connectPrefer: 'native',
      discoverAvailable: true,
      probe: 'refused',
    }),
  ).toEqual({
    kind: 'session',
    hostId: online.computer.hostId,
    fallback: 'macos',
  });

  const offline = hostEntry({ online: false, hostRunning: false });
  expect(
    decideRoute({
      entry: offline,
      adaptersAvailable: true,
      connectPrefer: 'native',
      discoverAvailable: true,
      probe: 'refused',
    }),
  ).toMatchObject({ kind: 'enableSharing', os: 'macos', entryKey: 'host:h1' });
  expect(
    decideRoute({
      entry: offline,
      adaptersAvailable: true,
      connectPrefer: 'native',
      discoverAvailable: true,
      probe: 'timeout',
    }),
  ).toEqual({ kind: 'session', hostId: offline.computer.hostId });
});

it('uses the first IPv4 address for tailnet and manual addresses for fallback', () => {
  const tailnet: ComputerEntry = {
    kind: 'tailnet',
    key: 'tailnet:n1',
    device: {
      node: 'n1',
      name: 'Tailnet Mac',
      os: 'macos',
      addrs: ['fd7a:115c::7', '100.64.0.7'],
      online: true,
      lastSeenAt: null,
    },
  };
  expect(
    decideRoute({
      entry: tailnet,
      adaptersAvailable: true,
      builtinProtocols: ['vnc'],
    }),
  ).toMatchObject({
    kind: 'adapter',
    target: { addr: '100.64.0.7' },
    addrKind: 'tailnet',
  });

  const manual: ComputerEntry = {
    kind: 'manual',
    key: 'manual:m1',
    manual: {
      id: 'm1',
      kind: 'computer',
      host: '192.168.31.50',
      port: 3389,
      hostId: null,
      name: 'Windows',
      networkId: null,
      addedAtUs: 1,
      os: 'windows',
    },
  };
  expect(
    decideRoute({
      entry: manual,
      adaptersAvailable: true,
      builtinProtocols: ['vnc'],
    }),
  ).toEqual({
    kind: 'fallback',
    target: { os: 'windows', addr: '192.168.31.50', port: 3389 },
  });
});
