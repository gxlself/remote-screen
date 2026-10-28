import type { DeviceCardProps } from '../../components/DeviceCard';
import type { Computer, Micros, NetPath } from '../../native/rscore/types';
import { common } from '../../i18n/zh/common';
import { directory } from '../../i18n/zh/directory';
import { native } from '../../i18n/zh/native';
import { formatRelativePast } from '../format/time';
import { entryName, entryOs, entryProtocol } from '../native/entries';
import type { ComputerEntry, EntryReach } from '../native/types';
import type { NetVerdict } from '../network/judge';
import type { SessionRecord } from '../session/store';

export type CardInput = {
  computer: Computer;
  nowUs: Micros;
  hostPath?: {
    path: NetPath;
    latencyMs: number;
    derpRegion?: string;
  };
  liveSession?: Pick<SessionRecord, 'phase' | 'role'>;
  verdict: NetVerdict;
  minHostVersion: string;
  /** Settings › 外网访问; off, an unreachable tailnet is not named. */
  tailnetEnabled?: boolean;
};

export type CardModel = Omit<
  DeviceCardProps,
  'onPress' | 'onLongPress' | 'thumbnail'
> & {
  /** Absent on Host cards; tells them apart from `NativeCardModel`. */
  kind?: 'host';
  hostId: string;
  group: 0 | 1 | 2 | 3 | 4;
  tap: { kind: 'open' } | { kind: 'error'; code: 170 };
  thumbnailUrl: string | null;
};

/** V10: a tailnet or manual computer without Host (system protocol only). */
export type NativeEntry = Exclude<ComputerEntry, { kind: 'host' }>;

export type NativeCardModel = {
  kind: 'native';
  key: string;
  entry: NativeEntry;
  /** Same scale as `CardModel.group`: online, not probed yet, offline. */
  group: 1 | 2 | 4;
  name: string;
  os: 'macos' | 'windows';
  look: 'live' | 'offline';
  reach: EntryReach;
  /** OS and system tool, `native.home.subtitle`. */
  subtitle: string;
  reachText: string;
  badge: string;
  address: string;
};

export type HomeCard = CardModel | NativeCardModel;

const SLEEPING_WINDOW_US = 30 * 60 * 1_000_000;
const IPV4 = /^\d{1,3}(?:\.\d{1,3}){3}$/;
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

function onlyTailnetAddresses(computer: Computer): boolean {
  return (
    computer.addresses.length > 0 &&
    computer.addresses.every(address => address.kind === 'tailnet')
  );
}

function thumbnailUrl(computer: Computer): string | null {
  return computer.thumbnailUrl === '' ? null : computer.thumbnailUrl;
}

function applyHostPath(
  card: CardModel,
  hostPath: NonNullable<CardInput['hostPath']>,
): void {
  card.path = hostPath.path;
  card.latencyMs = hostPath.latencyMs;
  if (hostPath.path === 'tailnetDerp' && hostPath.derpRegion) {
    card.a11yPath = directory.viaRelay(hostPath.derpRegion);
  }
}

function base(computer: Computer, group: CardModel['group']): CardModel {
  return {
    hostId: computer.hostId,
    group,
    tap: { kind: 'open' },
    thumbnailUrl: thumbnailUrl(computer),
    name: computer.name,
    look: 'live',
    displays: computer.displays.length,
  };
}

export function compareVersion(a: string, b: string): number {
  const left = SEMVER.exec(a);
  const right = SEMVER.exec(b);
  if (left === null || right === null) {
    return 0;
  }

  for (let index = 1; index <= 3; index += 1) {
    const difference = Number(left[index]) - Number(right[index]);
    if (difference !== 0) {
      return difference < 0 ? -1 : 1;
    }
  }

  const leftPre = left[4];
  const rightPre = right[4];
  if (leftPre === undefined || rightPre === undefined) {
    if (leftPre === rightPre) {
      return 0;
    }
    return leftPre === undefined ? 1 : -1;
  }

  const leftParts = leftPre.split('.');
  const rightParts = rightPre.split('.');
  const length = Math.max(leftParts.length, rightParts.length);
  for (let index = 0; index < length; index += 1) {
    const leftPart = leftParts[index];
    const rightPart = rightParts[index];
    if (leftPart === undefined || rightPart === undefined) {
      return leftPart === undefined ? -1 : 1;
    }
    if (leftPart === rightPart) {
      continue;
    }

    const leftNumeric = /^\d+$/.test(leftPart);
    const rightNumeric = /^\d+$/.test(rightPart);
    if (leftNumeric && rightNumeric) {
      return Number(leftPart) < Number(rightPart) ? -1 : 1;
    }
    if (leftNumeric !== rightNumeric) {
      return leftNumeric ? -1 : 1;
    }
    return leftPart < rightPart ? -1 : 1;
  }
  return 0;
}

export function cardModel(input: CardInput): CardModel {
  const { computer, hostPath, liveSession, verdict } = input;

  if (liveSession !== undefined && liveSession.phase !== 'ended') {
    const card = base(computer, 0);
    card.chip = { label: directory.inSession, tone: 'accent' };
    if (hostPath !== undefined) {
      applyHostPath(card, hostPath);
    }
    return card;
  }

  if (computer.online && computer.hostRunning) {
    const card = base(computer, 1);
    if (compareVersion(computer.hostVersion, input.minHostVersion) < 0) {
      card.chip = { label: directory.needsUpdate, tone: 'relay' };
      card.tap = { kind: 'error', code: 170 };
      if (hostPath !== undefined) {
        applyHostPath(card, hostPath);
      } else {
        card.statusText = directory.online;
      }
      return card;
    }

    if (verdict.tailnet === 'down' && onlyTailnetAddresses(computer)) {
      card.statusText =
        input.tailnetEnabled === false
          ? directory.otherNetwork
          : directory.needsTailnet;
      card.statusTone = 'muted';
      return card;
    }

    if (hostPath !== undefined) {
      applyHostPath(card, hostPath);
      card.live = card.thumbnailUrl !== null;
      return card;
    }

    card.statusText = directory.online;
    return card;
  }

  if (computer.online) {
    const card = base(computer, 2);
    // Screen Sharing / Remote Desktop still work without Host, so this is a
    // note rather than a warning.
    card.statusText = directory.hostNotRunning;
    card.statusTone = 'muted';
    return card;
  }

  const elapsedUs = input.nowUs - computer.lastSeenAt;
  if (elapsedUs <= SLEEPING_WINDOW_US) {
    const card = base(computer, 3);
    card.look = 'sleeping';
    card.statusText = directory.lastSeen(
      formatRelativePast(computer.lastSeenAt, input.nowUs),
    );
    card.statusTone = 'muted';
    return card;
  }

  const card = base(computer, 4);
  card.look = 'offline';
  card.statusText = directory.lastSeen(
    formatRelativePast(computer.lastSeenAt, input.nowUs),
  );
  card.statusTone = 'muted';
  return card;
}

/** Tailnet entries carry their own state; manual ones come from `useManualReach`. */
export function entryReach(
  entry: NativeEntry,
  manualReach: Readonly<Record<string, EntryReach>>,
): EntryReach {
  if (entry.kind === 'tailnet') {
    return entry.device.online ? 'online' : 'offline';
  }
  return manualReach[entry.key] ?? 'unknown';
}

export function entryAddress(entry: NativeEntry): string {
  if (entry.kind === 'tailnet') {
    const { addrs } = entry.device;
    return addrs.find(addr => IPV4.test(addr)) ?? addrs[0] ?? '';
  }
  const { host, port } = entry.manual;
  const defaultPort = entryProtocol(entry) === 'vnc' ? 5900 : 3389;
  if (port === defaultPort) {
    return host;
  }
  return host.includes(':') ? `[${host}]:${port}` : `${host}:${port}`;
}

export function reachText(reach: EntryReach): string {
  switch (reach) {
    case 'online':
      return common.status.online;
    case 'offline':
      return common.status.offline;
    case 'unknown':
      return native.home.reachUnknown;
  }
}

export function nativeCardModel(
  entry: NativeEntry,
  reach: EntryReach,
): NativeCardModel {
  const os = entryOs(entry);
  return {
    kind: 'native',
    key: entry.key,
    entry,
    group: reach === 'online' ? 1 : reach === 'unknown' ? 2 : 4,
    name: entryName(entry),
    os,
    look: reach === 'offline' ? 'offline' : 'live',
    reach,
    subtitle: native.home.subtitle(os),
    reachText: reachText(reach),
    badge: native.badge.native,
    address: entryAddress(entry),
  };
}

/** Host cards keep their host id; native cards use the entry key. */
export function cardKey(card: HomeCard): string {
  return card.kind === 'native' ? card.key : card.hostId;
}

export function sortCards<T extends { group: number }>(cards: T[]): T[] {
  return cards
    .map((card, index) => ({ card, index }))
    .sort(
      (left, right) =>
        left.card.group - right.card.group || left.index - right.index,
    )
    .map(item => item.card);
}

export function onlineCount(computers: Computer[]): number {
  return computers.filter(computer => computer.online).length;
}
