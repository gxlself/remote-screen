import type { Directory, OtherDevice } from '../../native/rscore/types';
import { directory as fixtureDirectory } from '../../native/rscore/fixtures';
import type { ManualComputer } from '../servers/manual';
import { buildEntries, entryName, entryOs, entryProtocol } from './entries';

const baseManual: ManualComputer = {
  id: 'manual-1',
  kind: 'computer',
  host: '192.168.31.50',
  port: 5900,
  hostId: null,
  name: 'Manual Mac',
  networkId: null,
  addedAtUs: 1,
  os: 'macos',
};

function directory(overrides: Partial<Directory> = {}): Directory {
  return {
    ...fixtureDirectory,
    computers: [],
    servers: [],
    others: [],
    ...overrides,
  };
}

function other(overrides: Partial<OtherDevice> = {}): OtherDevice {
  return {
    node: 'other-node',
    name: 'Other Mac',
    os: 'macos',
    addrs: ['100.64.0.50'],
    online: true,
    lastSeenAt: null,
    ...overrides,
  };
}

it('combines host, tailnet, and eligible manual computers', () => {
  const computer = fixtureDirectory.computers[0];
  const entries = buildEntries(
    directory({ computers: [computer], others: [other()] }),
    [baseManual],
  );

  expect(entries.map(entry => entry.kind)).toEqual([
    'host',
    'tailnet',
    'manual',
  ]);
  expect(entries.map(entry => entry.key)).toEqual([
    `host:${computer.hostId}`,
    'tailnet:other-node',
    'manual:manual-1',
  ]);
});

it('prefers directory entries when nodes or addresses overlap', () => {
  const computer = fixtureDirectory.computers[0];
  const sameNode = other({ node: computer.node });
  const sameComputerAddress: ManualComputer = {
    ...baseManual,
    id: 'manual-computer-address',
    host: computer.addresses[0].addr,
  };
  const sameOtherAddress: ManualComputer = {
    ...baseManual,
    id: 'manual-other-address',
    host: '100.64.0.50',
  };

  const entries = buildEntries(
    directory({ computers: [computer], others: [sameNode, other()] }),
    [sameComputerAddress, sameOtherAddress],
  );

  expect(entries.map(entry => entry.key)).toEqual([
    `host:${computer.hostId}`,
    'tailnet:other-node',
  ]);
});

it('filters old second-address records and records without an OS', () => {
  const missingOs = { ...baseManual, id: 'missing-os', os: undefined };
  const secondAddress = {
    ...baseManual,
    id: 'second-address',
    hostId: 'host-1',
  };
  expect(buildEntries(directory(), [missingOs, secondAddress])).toEqual([]);
});

it('keeps host order and sorts tailnet and manual groups', () => {
  const first = { ...fixtureDirectory.computers[0], name: 'Host z' };
  const second = { ...fixtureDirectory.computers[1], name: 'Host a' };
  const entries = buildEntries(
    directory({
      computers: [first, second],
      others: [
        other({ node: 'offline-z', name: 'Zed', online: false }),
        other({ node: 'online-z', name: 'Zoe', online: true }),
        other({ node: 'online-a', name: 'Amy', online: true }),
      ],
    }),
    [
      { ...baseManual, id: 'manual-z', name: 'Zed manual' },
      {
        ...baseManual,
        id: 'manual-a',
        name: 'Amy manual',
        host: '192.168.31.51',
      },
    ],
  );

  expect(entries.map(entry => entry.key)).toEqual([
    `host:${first.hostId}`,
    `host:${second.hostId}`,
    'tailnet:online-a',
    'tailnet:online-z',
    'tailnet:offline-z',
    'manual:manual-a',
    'manual:manual-z',
  ]);
});

it('provides names, OS values, and native protocols for every entry kind', () => {
  const computer = fixtureDirectory.computers[0];
  const entries = buildEntries(
    directory({ computers: [computer], others: [other()] }),
    [baseManual],
  );

  expect(entries.map(entryName)).toEqual([
    computer.name,
    'Other Mac',
    'Manual Mac',
  ]);
  expect(entries.map(entryOs)).toEqual(['macos', 'macos', 'macos']);
  expect(entries.map(entryProtocol)).toEqual(['vnc', 'vnc', 'vnc']);
  expect(
    entryProtocol({
      kind: 'manual',
      key: 'manual:rdp',
      manual: { ...baseManual, id: 'rdp', os: 'windows', protocol: 'rdp' },
    }),
  ).toBe('rdp');
});
