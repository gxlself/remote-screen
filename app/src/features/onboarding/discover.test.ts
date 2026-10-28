import { directory } from '../../native/rscore/fixtures';
import type { Directory, OtherDevice } from '../../native/rscore/types';
import type { ManualComputer } from '../servers/manual';
import {
  defaultDeviceName,
  discoverTitle,
  groupForDiscover,
  hasComputers,
  nativeSubtitle,
} from './discover';
import type { NativeEntry } from './discover';

const tailnetMac: OtherDevice = {
  node: 'home-mac',
  name: 'home-mac',
  os: 'macos',
  addrs: ['fd7a:115c:a1e0::30', '100.64.0.30'],
  online: true,
  lastSeenAt: null,
};

const savedPc: ManualComputer & { os: 'windows' } = {
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

const empty: Directory = {
  generatedAt: directory.generatedAt,
  computers: [],
  servers: [],
  others: [],
};

describe('groupForDiscover', () => {
  it('groups the fixture directory by heartbeat and SSH capability', () => {
    const groups = groupForDiscover(directory);
    expect(groups.remote).toHaveLength(5);
    expect(groups.ssh).toHaveLength(3);
    expect(groups.noHost).toHaveLength(0);
    expect(groups.total).toBe(8);
  });

  it('moves a computer without any heartbeat into noHost', () => {
    const withUninstalled: Directory = {
      ...directory,
      computers: directory.computers.map((computer, index) =>
        index === 0 ? { ...computer, lastHeartbeatAt: null } : computer,
      ),
    };
    const groups = groupForDiscover(withUninstalled);
    expect(groups.remote).toHaveLength(4);
    expect(groups.noHost).toHaveLength(1);
    expect(groups.noHost[0]?.hostId).toBe(directory.computers[0]?.hostId);
    expect(groups.total).toBe(8);
  });

  it('collects tailnet others and saved computers as native entries', () => {
    const groups = groupForDiscover({ ...directory, others: [tailnetMac] }, [
      savedPc,
    ]);
    expect(groups.nativeEntries.map(entry => entry.key)).toEqual([
      'tailnet:home-mac',
      `manual:${savedPc.id}`,
    ]);
    expect(groups.remote).toHaveLength(5);
    expect(groups.total).toBe(10);
  });

  it('leaves out a tailnet node that already runs the Host', () => {
    const studio = directory.computers[0];
    const groups = groupForDiscover({
      ...directory,
      others: [{ ...tailnetMac, node: studio?.node ?? '' }],
    });
    expect(groups.nativeEntries).toEqual([]);
    expect(groups.total).toBe(8);
  });
});

describe('hasComputers / discoverTitle', () => {
  it('counts native entries as computers', () => {
    const groups = groupForDiscover({ ...empty, others: [tailnetMac] });
    expect(hasComputers(groups)).toBe(true);
    expect(discoverTitle(groups)).toBe('找到了 1 台电脑');
  });

  it('uses the empty title when only SSH servers are found', () => {
    const groups = groupForDiscover({ ...empty, servers: directory.servers });
    expect(hasComputers(groups)).toBe(false);
    expect(discoverTitle(groups)).toBe('还没有电脑');
  });
});

describe('nativeSubtitle', () => {
  function tailnet(device: OtherDevice): NativeEntry {
    return { kind: 'tailnet', key: `tailnet:${device.node}`, device };
  }

  it('shows the IPv4 tailnet address of an online device', () => {
    expect(nativeSubtitle(tailnet(tailnetMac))).toBe('macOS · 100.64.0.30');
  });

  it('marks an offline tailnet device', () => {
    expect(nativeSubtitle(tailnet({ ...tailnetMac, online: false }))).toBe(
      'macOS · 离线',
    );
  });

  it('shows the saved address of a manual computer', () => {
    expect(
      nativeSubtitle({
        kind: 'manual',
        key: `manual:${savedPc.id}`,
        manual: savedPc,
      }),
    ).toBe('Windows · 192.168.31.88');
  });
});

describe('defaultDeviceName', () => {
  it('uses the local part of the Hub login name', () => {
    expect(defaultDeviceName('alice@', 'iPhone')).toBe('alice 的 iPhone');
  });

  it('falls back to the generic name without a user', () => {
    expect(defaultDeviceName(null, 'iPhone')).toBe('我的 iPhone');
    expect(defaultDeviceName('', 'iPhone')).toBe('我的 iPhone');
  });
});
