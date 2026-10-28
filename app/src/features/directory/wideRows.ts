import type {
  Computer,
  HostDetail,
  Micros,
  NetPath,
} from '../../native/rscore/types';
import { common } from '../../i18n/zh/common';
import { ipadHome } from '../../i18n/zh/ipadHome';
import type { HistoryEntry } from '../sessions/history';
import { formatClock, formatDurationWords } from '../format/duration';
import { formatMs, formatResolution } from '../format/latency';
import { formatHm, formatRelativePast } from '../format/time';
import type { CardModel } from './cardModel';

const US_PER_SECOND = 1_000_000;
const MS_PER_MINUTE = 60 * 1000;
const MS_PER_DAY = 24 * 60 * MS_PER_MINUTE;

function addressOfKind(
  computer: Computer,
  kind: Computer['addresses'][number]['kind'],
): string | null {
  return (
    computer.addresses.find(address => address.kind === kind)?.addr ?? null
  );
}

function tailnetAddress(computer: Computer): string | null {
  return (
    addressOfKind(computer, 'tailnet') ?? computer.addresses[0]?.addr ?? null
  );
}

function localDayIndex(us: Micros, tzOffsetMin?: number): number {
  const offset = tzOffsetMin ?? -new Date().getTimezoneOffset();
  return Math.floor(
    (Math.floor(us / 1000) + offset * MS_PER_MINUTE) / MS_PER_DAY,
  );
}

export function wideRowSubtitle(card: CardModel): string {
  const pathName = card.path === undefined ? null : common.path[card.path];

  if (card.chip !== undefined) {
    return pathName === null
      ? card.chip.label
      : `${pathName}${ipadHome.separator}${card.chip.label}`;
  }
  if (card.statusText !== undefined && card.statusText !== '') {
    return card.statusText;
  }
  if (pathName !== null && card.latencyMs !== undefined) {
    return `${pathName}${ipadHome.separator}${formatMs(card.latencyMs)}`;
  }
  return pathName ?? '';
}

export function pickDefault(
  cards: CardModel[],
  liveHostId: string | null,
): string | null {
  if (cards.length === 0) {
    return null;
  }
  if (liveHostId !== null) {
    const live = cards.find(card => card.hostId === liveHostId);
    if (live !== undefined) {
      return live.hostId;
    }
  }
  return cards.find(card => card.look !== 'offline')?.hostId ?? cards[0].hostId;
}

export function addressLine(computer: Computer): string {
  return [computer.node, tailnetAddress(computer), computer.osVersion]
    .filter((part): part is string => part !== undefined && part !== '')
    .join(ipadHome.separator);
}

export function routeAddr(
  computer: Computer,
  path: NetPath | null,
): string | null {
  switch (path) {
    case 'tailnetDirect':
    case 'tailnetDerp':
      return addressOfKind(computer, 'tailnet');
    case 'intranet':
      return addressOfKind(computer, 'intranet');
    case 'lan':
      return addressOfKind(computer, 'lan');
    default:
      return null;
  }
}

export function displaysCell(computer: Computer): {
  value: string;
  sub: string;
} {
  return {
    value: ipadHome.info.displayCount(computer.displays.length),
    sub: computer.displays
      .map(display => formatResolution(display.widthPx, display.heightPx))
      .join(ipadHome.separator),
  };
}

function permissionMark(granted: boolean): string {
  return granted ? ipadHome.info.permissionOn : ipadHome.info.permissionOff;
}

export function hostCell(
  computer: Computer,
  detail?: HostDetail,
): { value: string; sub: string } {
  return {
    value: `${computer.hostVersion}${ipadHome.separator}${
      computer.permsOk
        ? ipadHome.info.permissionsOk
        : ipadHome.info.permissionsMissing
    }`,
    sub:
      detail === undefined
        ? ''
        : `${ipadHome.info.screenPermission} ${permissionMark(
            detail.perms.screen,
          )} ${ipadHome.info.accessibilityPermission} ${permissionMark(
            detail.perms.accessibility,
          )}`,
  };
}

export function otherController(
  computer: Computer,
  nowUs: Micros,
): { deviceName: string; clock: string } | null {
  const controller = computer.sessions.find(
    session => session.role === 'controller',
  );
  if (controller === undefined) {
    return null;
  }
  return {
    deviceName: controller.deviceName,
    clock: formatClock((nowUs - controller.since) / US_PER_SECOND),
  };
}

/**
 * A Host session, or Screen Sharing / Remote Desktop opened from the same
 * computer (whose history target is its list key).
 */
export function isHostHistory(entry: HistoryEntry, hostId: string): boolean {
  return entry.kind === 'remote'
    ? entry.targetId === hostId
    : entry.kind === 'adapter' && entry.targetId === `host:${hostId}`;
}

export function lastConnectionCell(
  entries: HistoryEntry[],
  hostId: string,
  nowUs: Micros,
  tzOffsetMin?: number,
): { value: string; sub: string } | null {
  const entry = entries.find(candidate => isHostHistory(candidate, hostId));
  if (entry === undefined) {
    return null;
  }
  const today =
    localDayIndex(entry.endedAtUs, tzOffsetMin) ===
    localDayIndex(nowUs, tzOffsetMin);
  return {
    value: today
      ? ipadHome.info.today(formatHm(entry.endedAtUs, tzOffsetMin))
      : formatRelativePast(entry.endedAtUs, nowUs, tzOffsetMin),
    sub: formatDurationWords(entry.durationS),
  };
}
