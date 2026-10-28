import type { Directory, SshServer } from '../../native/rscore/types';
import { filesBrowse } from '../../i18n/zh/filesBrowse';
import { common } from '../../i18n/zh/common';
import { formatBytes } from '../format/misc';
import { formatDayHeader, formatHm } from '../format/time';
import {
  sourceKey,
  type FileSource,
  type Listing,
  type RemoteEntry,
  type Transfer,
} from './filesApi';
import { fileIcon, percent } from './transferView';

export type SourceChip = {
  key: string;
  source: FileSource;
  label: string;
  kind: 'computer' | 'server';
};

function serverIsOffline(server: SshServer, dir: Directory | undefined) {
  if (server.source === 'manual' || dir === undefined) {
    return false;
  }
  const listed = dir.servers.find(item => item.node === server.name);
  return listed !== undefined && listed.online === false;
}

export function fileSources(
  dir: Directory | undefined,
  servers: SshServer[],
): SourceChip[] {
  const computers =
    dir?.computers
      .filter(item => item.online && item.hostRunning && item.paired)
      .map(item => ({
        key: sourceKey({ kind: 'computer', hostId: item.hostId }),
        source: { kind: 'computer' as const, hostId: item.hostId },
        label: item.name,
        kind: 'computer' as const,
      })) ?? [];
  const remotes = servers
    .filter(
      server => server.source === 'manual' || !serverIsOffline(server, dir),
    )
    .map(server => ({
      key: sourceKey({ kind: 'server', serverId: server.serverId }),
      source: { kind: 'server' as const, serverId: server.serverId },
      label: server.name,
      kind: 'server' as const,
    }));
  return [...computers, ...remotes];
}

export function sourceName(
  src: FileSource,
  dir: Directory | undefined,
  servers: SshServer[],
): string {
  if (src.kind === 'computer') {
    return (
      dir?.computers.find(item => item.hostId === src.hostId)?.name ??
      src.hostId
    );
  }
  return (
    servers.find(item => item.serverId === src.serverId)?.name ?? src.serverId
  );
}

export function parentPath(path: string): string {
  const normalized = path.replace(/\/+$/, '');
  if (normalized === '') {
    return '/';
  }
  const index = normalized.lastIndexOf('/');
  if (index <= 0) {
    return '/';
  }
  return normalized.slice(0, index);
}

export function joinPath(dir: string, name: string): string {
  return `${dir.replace(/\/+$/, '')}/${name.replace(/^\/+/, '')}`;
}

export function crumbs(path: string): {
  parent: string | null;
  current: string;
  canGoUp: boolean;
} {
  const normalized = path.replace(/\/+$/, '');
  if (normalized === '' || normalized === '/') {
    return { parent: null, current: '/', canGoUp: false };
  }
  const index = normalized.lastIndexOf('/');
  const current = normalized.slice(index + 1);
  const before = normalized.slice(0, index);
  return {
    parent: before === '' ? '/' : before.slice(before.lastIndexOf('/') + 1),
    current,
    canGoUp: true,
  };
}

export function dayTime(
  us: number,
  nowUs: number,
  tzOffsetMin?: number,
): string {
  const day = formatDayHeader(us, nowUs, tzOffsetMin);
  const hm = formatHm(us, tzOffsetMin);
  if (day.startsWith(common.time.today)) {
    return `${common.time.today} ${hm}`;
  }
  if (day.startsWith(common.time.yesterday)) {
    return `${common.time.yesterday} ${hm}`;
  }
  return day.split(' ', 1)[0] ?? day;
}

export type EntryRowModel = {
  key: string;
  name: string;
  kind: 'dir' | 'file';
  icon: 'folder' | 'screenshot' | 'file';
  subtitle: string;
  badge: { text: string; tone: 'good' | 'accent' } | null;
  fullPath: string;
  localUri: string | null;
  a11y: string;
};

function entryKind(entry: RemoteEntry): 'dir' | 'file' {
  return entry.kind === 'dir' ? 'dir' : 'file';
}

function newerFirst(a: RemoteEntry, b: RemoteEntry): number {
  if (a.mtimeUs === null && b.mtimeUs === null) {
    return a.name.localeCompare(b.name);
  }
  if (a.mtimeUs === null) {
    return 1;
  }
  if (b.mtimeUs === null) {
    return -1;
  }
  return b.mtimeUs - a.mtimeUs || a.name.localeCompare(b.name);
}

function entryBadge(
  transfer: Transfer | undefined,
): { text: string; tone: 'good' | 'accent' } | null {
  if (transfer === undefined) {
    return null;
  }
  if (transfer.state === 'completed') {
    return { text: filesBrowse.badges.downloaded, tone: 'good' };
  }
  if (['queued', 'running', 'paused', 'verifying'].includes(transfer.state)) {
    return {
      text: `${percent(transfer.done, transfer.size)}%`,
      tone: 'accent',
    };
  }
  return null;
}

function fileSubtitle(entry: RemoteEntry, nowUs: number, tzOffsetMin?: number) {
  const size = entry.size === null ? '' : formatBytes(entry.size);
  const time =
    entry.mtimeUs === null ? '' : dayTime(entry.mtimeUs, nowUs, tzOffsetMin);
  return [size, time].filter(Boolean).join(' · ');
}

export function entryRows(
  listing: Listing,
  src: FileSource,
  transfers: Transfer[],
  nowUs: number,
  tzOffsetMin?: number,
): EntryRowModel[] {
  const dirs = listing.entries
    .filter(entry => entry.kind === 'dir')
    .sort(newerFirst);
  const files = listing.entries
    .filter(entry => entry.kind !== 'dir')
    .sort(newerFirst);

  return [...dirs, ...files].map(entry => {
    const kind = entryKind(entry);
    const fullPath = joinPath(listing.path, entry.name);
    if (kind === 'dir') {
      const subtitle =
        entry.childCount === null
          ? filesBrowse.entry.folder
          : filesBrowse.entry.items(entry.childCount);
      return {
        key: fullPath,
        name: entry.name,
        kind,
        icon: 'folder',
        subtitle,
        badge: null,
        fullPath,
        localUri: null,
        a11y: filesBrowse.a11y.folder(entry.name, subtitle),
      };
    }

    const subtitle = fileSubtitle(entry, nowUs, tzOffsetMin);
    const transfer = transfers.find(
      item =>
        item.direction === 'download' &&
        sourceKey(item.source) === sourceKey(src) &&
        item.remotePath === fullPath,
    );
    const badge = entryBadge(transfer);
    return {
      key: fullPath,
      name: entry.name,
      kind,
      icon: fileIcon(entry.name),
      subtitle,
      badge,
      fullPath,
      localUri: badge?.tone === 'good' ? transfer?.localUri ?? null : null,
      a11y: filesBrowse.a11y.file(
        entry.name,
        subtitle,
        badge === null ? null : badge.text,
      ),
    };
  });
}

export type BrowseLocation = {
  source: FileSource;
  path: string;
  label: string;
};

export type SendTarget = {
  key: string;
  source: FileSource;
  dir: string | null;
  title: string;
  subtitle: string;
  kind: 'computer' | 'server';
};

export function sendTargets(
  chips: SourceChip[],
  current: BrowseLocation | null,
): SendTarget[] {
  const currentTarget =
    current === null
      ? []
      : [
          {
            key: 'current',
            source: current.source,
            dir: current.path,
            title: current.label,
            subtitle: filesBrowse.send.current(crumbs(current.path).current),
            kind: current.source.kind,
          },
        ];
  return [
    ...currentTarget,
    ...chips.map(chip => ({
      key: chip.key,
      source: chip.source,
      dir: null,
      title: chip.label,
      subtitle:
        chip.kind === 'computer'
          ? filesBrowse.send.computer
          : filesBrowse.send.server,
      kind: chip.kind,
    })),
  ];
}
