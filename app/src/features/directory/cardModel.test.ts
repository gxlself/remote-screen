import {
  FIXTURE_NOW,
  MIN_HOST_VERSION,
  computers,
} from '../../native/rscore/fixtures';
import type { Computer } from '../../native/rscore/types';
import { directory } from '../../i18n/zh/directory';
import type { NetVerdict } from '../network/judge';
import {
  cardKey,
  cardModel,
  compareVersion,
  entryAddress,
  entryReach,
  nativeCardModel,
  onlineCount,
  sortCards,
} from './cardModel';
import type { CardModel, HomeCard, NativeEntry } from './cardModel';

const MINUTE_US = 60_000_000;

const verdict = (overrides: Partial<NetVerdict> = {}): NetVerdict => ({
  tailnet: 'up',
  tailnetAddr: '100.64.0.21',
  companyVpn: null,
  atCompany: null,
  banner: 'none',
  ...overrides,
});

function computer(hostId: string): Computer {
  const found = computers.find(item => item.hostId === hostId);
  if (found === undefined) {
    throw new Error(`Missing fixture ${hostId}`);
  }
  return found;
}

const studio = computer(computers[0].hostId);
const office = computer(computers[1].hostId);
const macbook = computer(computers[2].hostId);
const homePc = computer(computers[3].hostId);
const labMac = computer(computers[4].hostId);

describe('cardModel decision table', () => {
  it('shows a live session with the path and accent chip', () => {
    const model = cardModel({
      computer: studio,
      nowUs: FIXTURE_NOW,
      hostPath: { path: 'tailnetDirect', latencyMs: 6 },
      liveSession: { phase: 'streaming', role: 'controller' },
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('live');
    expect(model.path).toBe('tailnetDirect');
    expect(model.latencyMs).toBe(6);
    expect(model.chip).toEqual({ label: '会话中', tone: 'accent' });
    expect(model.group).toBe(0);
    expect(model.tap).toEqual({ kind: 'open' });
  });

  it('shows a session without a path when none is available', () => {
    const model = cardModel({
      computer: studio,
      nowUs: FIXTURE_NOW,
      liveSession: { phase: 'connecting', role: 'viewer' },
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.path).toBeUndefined();
    expect(model.statusText).toBeUndefined();
    expect(model.chip?.label).toBe('会话中');
    expect(model.group).toBe(0);
  });

  it('marks an old online host as needing an update and an E170 tap', () => {
    const oldStudio: Computer = { ...studio, hostVersion: '0.2.4' };
    const model = cardModel({
      computer: oldStudio,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('live');
    expect(model.chip).toEqual({ label: '需更新', tone: 'relay' });
    expect(model.tap).toEqual({ kind: 'error', code: 170 });
    expect(model.group).toBe(1);
    expect(model.statusText).toBe('在线');
  });

  it('shows a tailnet requirement when only tailnet addresses are usable', () => {
    const tailnetOnly: Computer = {
      ...studio,
      addresses: [{ kind: 'tailnet', addr: '100.64.0.7', port: 7400 }],
    };
    const model = cardModel({
      computer: tailnetOnly,
      nowUs: FIXTURE_NOW,
      verdict: verdict({ tailnet: 'down', tailnetAddr: null }),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('live');
    expect(model.statusText).toBe('需要 Tailscale');
    expect(model.statusTone).toBe('muted');
    expect(model.group).toBe(1);
  });

  it('shows the path, latency, TCP flag, relay label and thumbnail overlay', () => {
    const model = cardModel({
      computer: studio,
      nowUs: FIXTURE_NOW,
      hostPath: {
        path: 'tailnetDerp',
        latencyMs: 42,
        derpRegion: '上海',
      },
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('live');
    expect(model.path).toBe('tailnetDerp');
    expect(model.latencyMs).toBe(42);
    expect(model.a11yPath).toBe('经上海中继');
    expect(model.live).toBe(true);
    expect(model.group).toBe(1);
  });

  it('shows online when the host runs but no path is known', () => {
    const model = cardModel({
      computer: studio,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('live');
    expect(model.statusText).toBe(directory.online);
    expect(model.group).toBe(1);
  });

  it('shows Host 未运行 while the host is offline', () => {
    const model = cardModel({
      computer: labMac,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('live');
    expect(model.statusText).toBe('Host 未运行');
    // H47: system sharing still connects, so it is not a warning.
    expect(model.statusTone).toBe('muted');
    expect(model.group).toBe(2);
  });

  it('shows sleeping for a device last seen within 30 minutes', () => {
    const model = cardModel({
      computer: macbook,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('sleeping');
    expect(model.statusText).toBe('20 分钟前在线');
    expect(model.group).toBe(3);
  });

  it('shows offline for a device last seen over 30 minutes ago', () => {
    const model = cardModel({
      computer: homePc,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.look).toBe('offline');
    expect(model.statusText).toBe('2 小时前在线');
    expect(model.group).toBe(4);
  });

  it('keeps a session ahead of an update and host-not-running state', () => {
    const oldLab: Computer = {
      ...labMac,
      hostVersion: '0.1.0',
    };
    const model = cardModel({
      computer: oldLab,
      nowUs: FIXTURE_NOW,
      liveSession: { phase: 'streaming', role: 'viewer' },
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.group).toBe(0);
    expect(model.chip?.label).toBe('会话中');
  });

  it('treats the 30 minute boundary as sleeping', () => {
    const boundary: Computer = {
      ...macbook,
      lastSeenAt: FIXTURE_NOW - 30 * MINUTE_US,
    };
    expect(
      cardModel({
        computer: boundary,
        nowUs: FIXTURE_NOW,
        verdict: verdict(),
        minHostVersion: MIN_HOST_VERSION,
      }).look,
    ).toBe('sleeping');
  });

  it('uses displays count and null thumbnail when empty', () => {
    const model = cardModel({
      computer: office,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(model.displays).toBe(1);
    expect(model.thumbnailUrl).toBeNull();
  });
});

describe('compareVersion', () => {
  it.each([
    ['0.2.4', '0.3.0', -1],
    ['0.3.0', '0.3.0', 0],
    ['0.3.0-beta.1', '0.3.0', -1],
    ['0.10.0', '0.9.9', 1],
    ['0.3.0-beta.2', '0.3.0-beta.10', -1],
    ['0.3.0-alpha', '0.3.0-beta', -1],
  ])('compares %s to %s as %s', (left, right, expected) => {
    expect(compareVersion(left, right)).toBe(expected);
  });
});

describe('sortCards and onlineCount', () => {
  it('sorts by group stably', () => {
    const make = (hostId: string, group: CardModel['group']): CardModel => ({
      hostId,
      group,
      tap: { kind: 'open' },
      thumbnailUrl: null,
      name: hostId,
      look: 'live',
    });
    const cards = [
      make('offline-1', 4),
      make('live-1', 1),
      make('sleeping-1', 3),
      make('live-2', 1),
      make('session-1', 0),
    ];
    expect(sortCards(cards).map(card => card.hostId)).toEqual([
      'session-1',
      'live-1',
      'live-2',
      'sleeping-1',
      'offline-1',
    ]);
  });

  it('counts fixture computers that are online', () => {
    expect(onlineCount(computers)).toBe(3);
  });
});

describe('native cards (V10)', () => {
  const tailnetMac: NativeEntry = {
    kind: 'tailnet',
    key: 'tailnet:office-mac',
    device: {
      node: 'office-mac',
      name: 'Office Mac',
      os: 'macos',
      addrs: ['fd7a:115c::40', '100.64.0.40'],
      online: true,
      lastSeenAt: null,
    },
  };
  const manualPc: Extract<NativeEntry, { kind: 'manual' }> = {
    kind: 'manual',
    key: 'manual:lab-pc',
    manual: {
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
    },
  };

  it('models an online tailnet Mac with the system-sharing badge', () => {
    const card = nativeCardModel(tailnetMac, entryReach(tailnetMac, {}));
    expect(card).toMatchObject({
      kind: 'native',
      key: 'tailnet:office-mac',
      group: 1,
      name: 'Office Mac',
      look: 'live',
      reach: 'online',
      subtitle: 'Mac · 屏幕共享',
      reachText: '在线',
      badge: '系统共享',
      address: '100.64.0.40',
    });
  });

  it('says it is checking, not offline, before a manual computer is probed', () => {
    const card = nativeCardModel(manualPc, entryReach(manualPc, {}));
    expect(card.reach).toBe('unknown');
    expect(card.reachText).toBe('检测中…');
    expect(card.look).toBe('live');
    expect(card.group).toBe(2);
    expect(card.subtitle).toBe('Windows · 远程桌面');
  });

  it('dims an unreachable manual computer and sorts it last', () => {
    const card = nativeCardModel(
      manualPc,
      entryReach(manualPc, { 'manual:lab-pc': 'offline' }),
    );
    expect(card.look).toBe('offline');
    expect(card.reachText).toBe('离线');
    expect(card.group).toBe(4);

    const host = cardModel({
      computer: labMac,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    const online = nativeCardModel(tailnetMac, 'online');
    const sorted: HomeCard[] = sortCards<HomeCard>([card, host, online]);
    expect(sorted.map(cardKey)).toEqual([
      'tailnet:office-mac',
      labMac.hostId,
      'manual:lab-pc',
    ]);
  });

  it('adds the port to a manual address only when it is not the default', () => {
    expect(entryAddress(manualPc)).toBe('192.168.31.50');
    expect(
      entryAddress({
        ...manualPc,
        manual: { ...manualPc.manual, port: 13389 },
      }),
    ).toBe('192.168.31.50:13389');
  });

  it('keys Host cards by host id', () => {
    const host = cardModel({
      computer: studio,
      nowUs: FIXTURE_NOW,
      verdict: verdict(),
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(cardKey(host)).toBe(studio.hostId);
  });
});
