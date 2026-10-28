import type { HostPath } from '../directory/pathStore';
import { formatMs } from '../format/latency';
import { buildEntries, entryOs } from '../native/entries';
import type { ComputerEntry } from '../native/types';
import type { ManualComputer } from '../servers/manual';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import { onboardDiscover } from '../../i18n/zh/onboardDiscover';
import type { Computer, Directory, Server } from '../../native/rscore/types';

/** V10: a computer without the Host, reached with the system protocol. */
export type NativeEntry = Exclude<ComputerEntry, { kind: 'host' }>;

export type DiscoverGroups = {
  /** Tailnet `others` and saved computers; listed first (V10 C8). */
  nativeEntries: NativeEntry[];
  remote: Computer[];
  ssh: Server[];
  noHost: Computer[];
  total: number;
};

export function groupForDiscover(
  directory: Directory,
  manual: ManualComputer[] = [],
): DiscoverGroups {
  const nativeEntries = buildEntries(directory, manual).filter(
    (entry): entry is NativeEntry => entry.kind !== 'host',
  );
  const remote = directory.computers.filter(
    computer => computer.lastHeartbeatAt !== null,
  );
  const noHost = directory.computers.filter(
    computer => computer.lastHeartbeatAt === null,
  );
  const ssh = directory.servers;
  return {
    nativeEntries,
    remote,
    ssh,
    noHost,
    total: nativeEntries.length + remote.length + ssh.length + noHost.length,
  };
}

/** True when there is a computer to show; SSH servers alone do not count. */
export function hasComputers(groups: DiscoverGroups): boolean {
  const { nativeEntries, remote, noHost } = groups;
  return nativeEntries.length + remote.length + noHost.length > 0;
}

export function discoverTitle(groups: DiscoverGroups): string {
  return hasComputers(groups)
    ? onboardDiscover.title(groups.total)
    : native.home.emptyTitle;
}

export function nativeSubtitle(entry: NativeEntry): string {
  const os = onboardDiscover.computer.osFallback[entryOs(entry)];
  if (entry.kind === 'manual') {
    return native.onboard.subtitle(os, entry.manual.host);
  }
  const { addrs, online } = entry.device;
  const addr = addrs.find(value => !value.includes(':')) ?? addrs[0];
  return online && addr !== undefined
    ? native.onboard.subtitle(os, addr)
    : native.onboard.subtitle(os, common.path.offline);
}

function loginName(user: string | null): string | null {
  const trimmed = user?.trim() ?? '';
  if (trimmed === '') {
    return null;
  }
  const localPart = trimmed.split('@')[0]?.trim() ?? '';
  return localPart === '' ? null : localPart;
}

export function defaultDeviceName(user: string | null, model: string): string {
  const name = loginName(user);
  return name === null
    ? onboardDiscover.deviceName.mine(model)
    : onboardDiscover.deviceName.withUser(name, model);
}

export function computerSubtitle(
  computer: Computer,
  hostPath?: Pick<HostPath, 'path' | 'latencyMs'>,
): string {
  const os =
    computer.osVersion ??
    (computer.os === 'macos'
      ? onboardDiscover.computer.osFallback.macos
      : onboardDiscover.computer.osFallback.windows);
  if (hostPath === undefined) {
    return os;
  }
  return onboardDiscover.computer.subtitle(
    os,
    common.path[hostPath.path],
    formatMs(hostPath.latencyMs),
  );
}

export function serverSubtitle(server: Server): string {
  return server.tailscaleSsh === true
    ? onboardDiscover.ssh.readySubtitle
    : onboardDiscover.ssh.keySubtitle(server.os);
}
