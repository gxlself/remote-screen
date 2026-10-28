import {
  FIXTURE_NOW,
  MIN_HOST_VERSION,
  computers,
  hostDetail,
} from '../../native/rscore/fixtures';
import type { Computer, HostDetail } from '../../native/rscore/types';
import { cardModel, type CardModel } from './cardModel';
import type { NetVerdict } from '../network/judge';
import {
  addressLine,
  displaysCell,
  hostCell,
  isHostHistory,
  otherController,
  pickDefault,
  routeAddr,
  wideRowSubtitle,
} from './wideRows';

const MINUTE_US = 60_000_000;
const HOUR_US = 60 * MINUTE_US;

const verdict: NetVerdict = {
  tailnet: 'up',
  tailnetAddr: '100.64.0.21',
  companyVpn: null,
  atCompany: null,
  banner: 'none',
};

function computer(name: string): Computer {
  const found = computers.find(item => item.name === name);
  if (found === undefined) {
    throw new Error(`Missing fixture ${name}`);
  }
  return found;
}

function card(
  input: Computer,
  options: {
    path?: 'tailnetDirect' | 'tailnetDerp';
    latencyMs?: number;
    live?: boolean;
  } = {},
): CardModel {
  return cardModel({
    computer: input,
    nowUs: FIXTURE_NOW,
    hostPath:
      options.path === undefined
        ? undefined
        : { path: options.path, latencyMs: options.latencyMs ?? 0 },
    liveSession:
      options.live === true
        ? { phase: 'streaming', role: 'controller' }
        : undefined,
    verdict,
    minHostVersion: MIN_HOST_VERSION,
  });
}

describe('wideRowSubtitle', () => {
  it('shows path and chip for a live session', () => {
    expect(
      wideRowSubtitle(
        card(computer('Studio Mac mini'), {
          path: 'tailnetDirect',
          latencyMs: 6,
          live: true,
        }),
      ),
    ).toBe('直连 · 会话中');
  });

  it('shows a relay path and latency', () => {
    expect(
      wideRowSubtitle(
        card(computer('工位 Windows'), { path: 'tailnetDerp', latencyMs: 48 }),
      ),
    ).toBe('中继 · 48 ms');
  });

  it('shows status text for a recently seen device', () => {
    const macbook = {
      ...computer('小明的 MacBook Pro'),
      lastSeenAt: FIXTURE_NOW - 20 * MINUTE_US,
    };
    expect(wideRowSubtitle(card(macbook))).toBe('20 分钟前在线');
  });

  it('shows status text for an older device', () => {
    const homePc = {
      ...computer('家里台式机'),
      lastSeenAt: FIXTURE_NOW - 2 * HOUR_US,
    };
    expect(wideRowSubtitle(card(homePc))).toBe('2 小时前在线');
  });

  it('shows only the chip when no path is known', () => {
    expect(
      wideRowSubtitle(card(computer('Studio Mac mini'), { live: true })),
    ).toBe('会话中');
  });
});

describe('pickDefault', () => {
  const online = card(computer('Studio Mac mini'));
  const offline = card(computer('家里台式机'));

  it('prefers the live host', () => {
    expect(pickDefault([online, offline], offline.hostId)).toBe(offline.hostId);
  });

  it('falls back to the first non-offline card', () => {
    expect(pickDefault([offline, online], null)).toBe(online.hostId);
  });

  it('falls back to the first card and null for an empty list', () => {
    expect(pickDefault([offline], null)).toBe(offline.hostId);
    expect(pickDefault([], null)).toBeNull();
  });
});

describe('address helpers', () => {
  it('builds the node, tailnet address and OS line', () => {
    expect(addressLine(computer('Studio Mac mini'))).toBe(
      'studio-mac · 100.64.0.7 · macOS 26.1',
    );
  });

  it('finds the address for a path', () => {
    expect(routeAddr(computer('Studio Mac mini'), 'lan')).toBe('192.168.31.20');
    expect(routeAddr(computer('Studio Mac mini'), null)).toBeNull();
  });
});

describe('info cells', () => {
  it('summarizes displays', () => {
    expect(displaysCell(computer('Studio Mac mini'))).toEqual({
      value: '2 台',
      sub: '2560×1600 · 2160×3840',
    });
  });

  it('marks missing permissions', () => {
    const studio = computer('Studio Mac mini');
    const detail: HostDetail = {
      ...hostDetail(studio.hostId),
      perms: { screen: true, accessibility: false },
    };
    expect(hostCell({ ...studio, permsOk: false }, detail)).toEqual({
      value: '0.3.1 · 缺少权限',
      sub: '屏幕录制 ✓ 辅助功能 ✗',
    });
  });

  it('omits the permission subline without host detail', () => {
    expect(hostCell(computer('Studio Mac mini')).sub).toBe('');
  });
});

describe('otherController', () => {
  it('returns the first controller with a formatted clock', () => {
    const studio = {
      ...computer('Studio Mac mini'),
      sessions: [
        {
          deviceName: '小明的 iPhone',
          role: 'controller' as const,
          since: FIXTURE_NOW - 2538_000_000,
        },
      ],
    };
    expect(otherController(studio, FIXTURE_NOW)).toEqual({
      deviceName: '小明的 iPhone',
      clock: '42:18',
    });
  });

  it('returns null without a controller', () => {
    expect(
      otherController(computer('Studio Mac mini'), FIXTURE_NOW),
    ).toBeNull();
  });
});

describe('isHostHistory', () => {
  const base = {
    id: 'h',
    name: 'Studio Mac mini',
    startedAtUs: 0,
    endedAtUs: 1,
    durationS: 1,
  };

  it('counts Host sessions and screen sharing with the same computer', () => {
    expect(
      isHostHistory({ ...base, kind: 'remote', targetId: 'host-1' }, 'host-1'),
    ).toBe(true);
    expect(
      isHostHistory(
        { ...base, kind: 'adapter', targetId: 'host:host-1', protocol: 'vnc' },
        'host-1',
      ),
    ).toBe(true);
  });

  it('skips terminals and other computers', () => {
    expect(
      isHostHistory({ ...base, kind: 'ssh', targetId: 'host-1' }, 'host-1'),
    ).toBe(false);
    expect(
      isHostHistory({ ...base, kind: 'remote', targetId: 'host-2' }, 'host-1'),
    ).toBe(false);
    expect(
      isHostHistory(
        { ...base, kind: 'adapter', targetId: 'tailnet:host-1' },
        'host-1',
      ),
    ).toBe(false);
  });
});
