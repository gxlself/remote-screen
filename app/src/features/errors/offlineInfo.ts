import { formatHm, formatRelativePast } from '../format/time';
import { ipInCidr } from '../network/cidr';
import { common } from '../../i18n/zh/common';
import type { Computer, Directory } from '../../native/rscore/types';

const US_PER_MS = 1000;
const MS_PER_MINUTE = 60 * 1000;
const MS_PER_DAY = 24 * 60 * MS_PER_MINUTE;

function localDayIndex(us: number): number {
  const localMs =
    Math.floor(us / US_PER_MS) - new Date().getTimezoneOffset() * MS_PER_MINUTE;
  return Math.floor(localMs / MS_PER_DAY);
}

function dayWord(us: number, nowUs: number): string {
  const offset = localDayIndex(nowUs) - localDayIndex(us);
  if (offset <= 0) {
    return common.time.today;
  }
  if (offset === 1) {
    return common.time.yesterday;
  }
  const date = new Date(
    Math.floor(us / US_PER_MS) - new Date().getTimezoneOffset() * MS_PER_MINUTE,
  );
  return common.time.monthDay(date.getUTCMonth() + 1, date.getUTCDate());
}

export function lastSeenAtText(us: number, nowUs: number): string {
  return `${dayWord(us, nowUs)} ${formatHm(us)}`;
}

function lanAddresses(computer: Computer): Computer['addresses'] {
  return computer.addresses.filter(address => address.kind === 'lan');
}

function sameLan(target: Computer, candidate: Computer): boolean {
  const targetAddresses = lanAddresses(target);
  const candidateAddresses = lanAddresses(candidate);
  return targetAddresses.some(targetAddress =>
    candidateAddresses.some(
      candidateAddress =>
        (targetAddress.cidr !== undefined &&
          candidateAddress.cidr !== undefined &&
          targetAddress.cidr === candidateAddress.cidr) ||
        (targetAddress.cidr !== undefined &&
          ipInCidr(candidateAddress.addr, targetAddress.cidr)) ||
        (candidateAddress.cidr !== undefined &&
          ipInCidr(targetAddress.addr, candidateAddress.cidr)),
    ),
  );
}

export function offlineInfo(
  host: Computer,
  dir: Directory,
  nowUs: number,
): {
  lastSeenRel: string;
  lastSeenAt: string;
  relays: Computer[];
} {
  return {
    lastSeenRel: formatRelativePast(host.lastSeenAt, nowUs),
    lastSeenAt: lastSeenAtText(host.lastSeenAt, nowUs),
    relays: dir.computers.filter(
      computer =>
        computer.hostId !== host.hostId &&
        computer.online &&
        computer.hostRunning &&
        sameLan(host, computer),
    ),
  };
}
