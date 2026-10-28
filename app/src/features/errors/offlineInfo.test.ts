import {
  computers,
  directory,
  FIXTURE_NOW,
} from '../../native/rscore/fixtures';
import type { Computer, Directory } from '../../native/rscore/types';
import { offlineInfo } from './offlineInfo';

const HOME_PC = 'd2n6c4v8b1x5za7e';
const STUDIO = 'h7v2m9q3k8x4tb1a';
const MACBOOK = 'm5q1z8r2k7v3xa9d';

function host(
  hostId: string,
  source: readonly Computer[] = computers,
): Computer {
  const found = source.find(computer => computer.hostId === hostId);
  if (found === undefined) {
    throw new Error(`Unknown host ${hostId}`);
  }
  return found;
}

function directoryWith(source: Computer[]): Directory {
  return { ...directory, computers: source };
}

describe('offlineInfo', () => {
  it('formats when the host was last online', () => {
    const nowUs = new Date(2026, 9, 21, 10, 0).getTime() * 1000;
    const home = {
      ...host(HOME_PC),
      lastSeenAt: new Date(2026, 9, 21, 8, 0).getTime() * 1000,
    };
    const info = offlineInfo(home, directoryWith([home]), nowUs);

    expect(info.lastSeenRel).toBe('2 小时前');
    expect(info.lastSeenAt).toBe('今天 08:00');
  });

  it('finds online hosts in the same LAN but skips offline ones', () => {
    const source = computers.map(computer =>
      computer.hostId === HOME_PC
        ? {
            ...computer,
            addresses: [
              ...computer.addresses,
              {
                kind: 'lan' as const,
                addr: '192.168.31.30',
                port: 7400,
                cidr: '192.168.31.0/24',
              },
            ],
          }
        : computer,
    );
    const info = offlineInfo(
      host(HOME_PC, source),
      directoryWith(source),
      FIXTURE_NOW,
    );

    expect(info.lastSeenRel).toBe('2 小时前');
    expect(info.relays.map(computer => computer.hostId)).toContain(STUDIO);
    expect(info.relays.map(computer => computer.hostId)).not.toContain(MACBOOK);
  });

  it('accepts a candidate whose LAN CIDR contains the target address', () => {
    const home = {
      ...host(HOME_PC),
      addresses: [
        ...host(HOME_PC).addresses,
        {
          kind: 'lan' as const,
          addr: '192.168.31.30',
          port: 7400,
        },
      ],
    };
    const studio = {
      ...host(STUDIO),
      addresses: [
        ...host(STUDIO).addresses,
        {
          kind: 'lan' as const,
          addr: '192.168.31.20',
          port: 7400,
          cidr: '192.168.31.0/24',
        },
      ],
    };
    const source = computers.map(computer =>
      computer.hostId === HOME_PC
        ? home
        : computer.hostId === STUDIO
        ? studio
        : computer,
    );
    const info = offlineInfo(home, directoryWith(source), FIXTURE_NOW);

    expect(info.relays.map(computer => computer.hostId)).toContain(STUDIO);
  });

  it('returns no relays when the host has no LAN address', () => {
    const info = offlineInfo(host(HOME_PC), directory, FIXTURE_NOW);

    expect(info.relays).toEqual([]);
  });
});
