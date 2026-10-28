import type { DevicePlatform, NetPath } from '../../native/rscore/types';
import { fileTransfers } from '../../i18n/zh/fileTransfers';
import { common } from '../../i18n/zh/common';
import { formatBytes } from '../format/misc';
import { formatDayHeader, formatHm } from '../format/time';
import type {
  FileSource,
  Transfer,
  TransferFail,
  TransferId,
} from './filesApi';

export type TransferNames = { source(src: FileSource): string; phone: string };
export type PhoneInfo = { platform: DevicePlatform; isPad: boolean };

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;
const ACTIVE_STATES = ['queued', 'running', 'paused', 'verifying'] as const;
const STALLED_STATES = ['interrupted', 'failed'] as const;

function isActive(state: Transfer['state']): boolean {
  return (ACTIVE_STATES as readonly string[]).includes(state);
}

function isStalled(state: Transfer['state']): boolean {
  return (STALLED_STATES as readonly string[]).includes(state);
}

function trailingSegment(path: string): string {
  const normalized = path.replace(/\/+$/, '');
  const parts = normalized.split('/');
  return parts[parts.length - 1] ?? '';
}

function parentSegment(path: string): string {
  const normalized = path.replace(/\/+$/, '');
  const index = normalized.lastIndexOf('/');
  return index <= 0 ? '' : trailingSegment(normalized.slice(0, index));
}

function oneDecimal(value: number): string {
  return value.toFixed(1).replace(/\.0$/, '');
}

function unitIndexFor(size: number): number {
  let value = Math.abs(size);
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < BYTE_UNITS.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return unitIndex;
}

function scaledInUnit(value: number, unitIndex: number): number {
  return value / 1024 ** unitIndex;
}

function defaultDirectory(source: FileSource): string {
  return source.kind === 'computer'
    ? fileTransfers.directories.computer
    : fileTransfers.directories.server;
}

function failReason(fail: TransferFail | null): string {
  if (fail === null) {
    return fileTransfers.failures.fallback;
  }
  return fileTransfers.failures[fail];
}

function spoken(text: string): string {
  return text
    .replaceAll(fileTransfers.separators.arrow, fileTransfers.spoken.to)
    .replaceAll(fileTransfers.separators.chevron, fileTransfers.spoken.comma)
    .replaceAll(fileTransfers.separators.dot, fileTransfers.spoken.comma);
}

function activeRoute(transfer: Transfer, names: TransferNames): string {
  const source = names.source(transfer.source);
  if (transfer.direction === 'upload') {
    const directory =
      transfer.remotePath === null
        ? defaultDirectory(transfer.source)
        : trailingSegment(transfer.remotePath);
    return fileTransfers.routes.upload(names.phone, source, directory);
  }
  return fileTransfers.routes.download(
    source,
    transfer.remotePath === null
      ? defaultDirectory(transfer.source)
      : parentSegment(transfer.remotePath),
  );
}

function stalledRoute(transfer: Transfer, names: TransferNames): string {
  const source = names.source(transfer.source);
  const pct = percent(transfer.done, transfer.size);
  if (transfer.direction === 'download') {
    return transfer.state === 'interrupted'
      ? fileTransfers.routes.interruptedDownload(source, names.phone, pct)
      : fileTransfers.routes.failedDownload(
          source,
          names.phone,
          failReason(transfer.fail),
        );
  }
  return transfer.state === 'interrupted'
    ? fileTransfers.routes.interruptedUpload(names.phone, source, pct)
    : fileTransfers.routes.failedUpload(
        names.phone,
        source,
        failReason(transfer.fail),
      );
}

function completedDestination(
  transfer: Transfer,
  names: TransferNames,
): string {
  if (transfer.savedTo === 'photos') {
    return fileTransfers.destinations.photos;
  }
  if (transfer.savedTo === 'files') {
    return fileTransfers.destinations.files;
  }
  return names.phone;
}

function doneText(transfer: Transfer, names: TransferNames): string {
  if (transfer.state !== 'completed') {
    return '';
  }
  const size = formatBytes(transfer.size);
  if (transfer.direction === 'download') {
    return fileTransfers.completed.download(
      names.source(transfer.source),
      completedDestination(transfer, names),
      size,
    );
  }
  return fileTransfers.completed.upload(
    names.phone,
    names.source(transfer.source),
    size,
  );
}

function speedText(transfer: Transfer): string {
  if (transfer.state === 'running') {
    return formatSpeed(transfer.bytesPerSec, transfer.size - transfer.done);
  }
  if (transfer.state === 'paused') {
    return fileTransfers.progress.paused;
  }
  return '';
}

function progressText(transfer: Transfer): string {
  if (transfer.state === 'running' || transfer.state === 'paused') {
    return formatProgress(transfer.done, transfer.size);
  }
  if (transfer.state === 'queued') {
    return fileTransfers.progress.waiting;
  }
  if (transfer.state === 'verifying') {
    return fileTransfers.progress.verifying;
  }
  return '';
}

function action(state: Transfer['state']): TransferRowModel['action'] {
  if (state === 'running' || state === 'queued') {
    return 'pause';
  }
  if (state === 'paused' || state === 'interrupted') {
    return 'resume';
  }
  if (state === 'failed') {
    return 'retry';
  }
  return null;
}

function tone(state: Transfer['state']): TransferRowModel['tone'] {
  if (state === 'interrupted') {
    return 'relay';
  }
  if (state === 'failed') {
    return 'bad';
  }
  return 'normal';
}

function accessibilityStatus(
  transfer: Transfer,
  names: TransferNames,
  time: string,
): string {
  if (transfer.state === 'running') {
    const progress = fileTransfers.a11y.progress(
      percent(transfer.done, transfer.size),
    );
    if (transfer.bytesPerSec <= 0) {
      return progress;
    }
    const eta = formatEta(
      Math.max(
        1,
        Math.round((transfer.size - transfer.done) / transfer.bytesPerSec),
      ),
    );
    return [progress, fileTransfers.a11y.remaining(eta)].join(
      fileTransfers.spoken.comma,
    );
  }
  if (transfer.state === 'paused') {
    return [
      fileTransfers.a11y.progress(percent(transfer.done, transfer.size)),
      fileTransfers.progress.paused,
    ].join(fileTransfers.spoken.comma);
  }
  if (transfer.state === 'queued') {
    return fileTransfers.progress.waiting;
  }
  if (transfer.state === 'verifying') {
    return fileTransfers.progress.verifying;
  }
  if (transfer.state === 'completed') {
    return [spoken(doneText(transfer, names)), time].join(
      fileTransfers.spoken.comma,
    );
  }
  return '';
}

export function phoneLabel(info: PhoneInfo): string {
  if (info.isPad || info.platform === 'ipados') {
    return fileTransfers.devices.ipad;
  }
  if (info.platform === 'android') {
    return fileTransfers.devices.phone;
  }
  return fileTransfers.devices.iphone;
}

export function fileIcon(name: string): 'screenshot' | 'file' {
  const index = name.lastIndexOf('.');
  const extension = index === -1 ? '' : name.slice(index + 1).toLowerCase();
  return ['png', 'jpg', 'jpeg', 'heic', 'gif', 'webp'].includes(extension)
    ? 'screenshot'
    : 'file';
}

export function percent(done: number, size: number): number {
  if (size <= 0) {
    return 100;
  }
  return Math.min(100, Math.max(0, Math.floor((done * 100) / size)));
}

export function formatProgress(done: number, size: number): string {
  const unitIndex = unitIndexFor(size);
  const unit = BYTE_UNITS[unitIndex];
  return fileTransfers.progress.value(
    oneDecimal(scaledInUnit(done, unitIndex)),
    oneDecimal(scaledInUnit(size, unitIndex)),
    unit,
    percent(done, size),
  );
}

export function formatEta(seconds: number): string {
  if (seconds < 60) {
    return fileTransfers.progress.seconds(seconds);
  }
  if (seconds < 3600) {
    return fileTransfers.progress.minutes(Math.round(seconds / 60));
  }
  const totalMinutes = Math.floor(seconds / 60);
  return common.time.hourMinute(
    Math.floor(totalMinutes / 60),
    totalMinutes % 60,
  );
}

export function formatSpeed(bytesPerSec: number, remaining: number): string {
  if (bytesPerSec <= 0) {
    return fileTransfers.progress.connecting;
  }
  const eta = formatEta(Math.max(1, Math.round(remaining / bytesPerSec)));
  return fileTransfers.progress.speed(formatBytes(bytesPerSec), eta);
}

export type TransferGroups = {
  active: Transfer[];
  stalled: Transfer[];
  doneToday: Transfer[];
  doneEarlier: Transfer[];
};

export function groupTransfers(
  transfers: Transfer[],
  nowUs: number,
  tzOffsetMin?: number,
): TransferGroups {
  const active = transfers
    .filter(transfer => isActive(transfer.state))
    .sort((a, b) => b.startedAt - a.startedAt);
  const stalled = transfers
    .filter(transfer => isStalled(transfer.state))
    .sort((a, b) => b.updatedAt - a.updatedAt);
  const completed = transfers
    .filter(transfer => transfer.state === 'completed')
    .sort((a, b) => b.updatedAt - a.updatedAt);
  const doneToday: Transfer[] = [];
  const doneEarlier: Transfer[] = [];
  for (const transfer of completed) {
    if (
      formatDayHeader(transfer.updatedAt, nowUs, tzOffsetMin).startsWith(
        common.time.today,
      )
    ) {
      doneToday.push(transfer);
    } else if (doneEarlier.length < 20) {
      doneEarlier.push(transfer);
    }
  }
  return { active, stalled, doneToday, doneEarlier };
}

export type TransferRowModel = {
  key: TransferId;
  title: string;
  icon: 'screenshot' | 'file';
  route: string;
  pathMark: NetPath | null;
  pathLabel: string | null;
  pct: number;
  progressText: string;
  speedText: string;
  action: 'pause' | 'resume' | 'retry' | null;
  tone: 'normal' | 'relay' | 'bad';
  doneText: string;
  time: string;
  a11y: string;
};

export function transferRow(
  transfer: Transfer,
  names: TransferNames,
  tzOffsetMin?: number,
): TransferRowModel {
  const route = isStalled(transfer.state)
    ? stalledRoute(transfer, names)
    : isActive(transfer.state)
    ? activeRoute(transfer, names)
    : '';
  const pathMark = isActive(transfer.state) ? transfer.netPath : null;
  const pathLabel = pathMark === null ? null : common.path[pathMark];
  const done = doneText(transfer, names);
  const time = formatHm(transfer.updatedAt, tzOffsetMin);
  const status = accessibilityStatus(transfer, names, time);
  const a11y = [transfer.name, spoken(route), pathLabel, status]
    .filter((part): part is string => part !== null && part.length > 0)
    .join(fileTransfers.spoken.comma);

  return {
    key: transfer.transferId,
    title: transfer.name,
    icon: fileIcon(transfer.name),
    route,
    pathMark,
    pathLabel,
    pct: percent(transfer.done, transfer.size),
    progressText: progressText(transfer),
    speedText: speedText(transfer),
    action: action(transfer.state),
    tone: tone(transfer.state),
    doneText: done,
    time,
    a11y,
  };
}
