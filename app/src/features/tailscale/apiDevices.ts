import type {
  Micros,
  OtherDevice,
  Server,
  TailscaleDevice,
} from '../../native/rscore/types';
import { parseIpv4 } from '../network/cidr';

/** Without connectedToControl, a device seen this recently counts as online. */
export const API_ONLINE_WINDOW_US = 5 * 60 * 1_000_000;

/** Host and Hub nodes are never listed from the tailnet itself. */
const SKIPPED_TAGS = ['tag:rs-host', 'tag:rs-hub'];

/** What the open ports say about a device without a reported system. */
export type ProbeClass = {
  /** 3389 open: Windows (remote desktop); otherwise 5900: a Mac (screen sharing). */
  computer: 'macos' | 'windows' | null;
  /** 22 open: an SSH server as well. */
  ssh: boolean;
};

/** By device id. */
export type TailnetProbes = Record<string, ProbeClass>;

function firstIpv4(addresses: readonly string[]): string {
  return addresses.find(ip => parseIpv4(ip) !== null) ?? addresses[0] ?? '';
}

export function apiDeviceOnline(device: TailscaleDevice, now: Micros): boolean {
  if (device.online !== null) {
    return device.online;
  }
  return (
    device.lastSeenAt !== null &&
    now - device.lastSeenAt <= API_ONLINE_WINDOW_US
  );
}

/**
 * Tailscale API devices for the Hub-less directory: Linux (and tag:server)
 * nodes are SSH servers, Mac and Windows go to others. Devices without a
 * system (Headscale) are listed by `probes` instead: as a Mac or Windows PC,
 * and also as an SSH server when 22 is open; offline or unprobed ones are
 * not. Phones, Hosts, the Hub and any device with an address in `taken` are
 * left out.
 */
export function apiDirectoryEntries(
  devices: TailscaleDevice[],
  taken: ReadonlySet<string>,
  now: Micros,
  probes: TailnetProbes = {},
): { servers: Server[]; others: OtherDevice[] } {
  const servers: Server[] = [];
  const others: OtherDevice[] = [];
  for (const device of devices) {
    const os = device.os;
    if (
      SKIPPED_TAGS.some(tag => device.tags.includes(tag)) ||
      device.addresses.some(ip => taken.has(ip))
    ) {
      continue;
    }
    if (os === 'unknown') {
      const probed: ProbeClass | undefined = probes[device.id];
      if (device.online === false || probed === undefined) {
        continue;
      }
      if (probed.ssh) {
        servers.push({
          node: device.name,
          addr: firstIpv4(device.addresses),
          online: true,
          os: probed.computer ?? 'linux',
          tailscaleSsh: null,
        });
      }
      if (probed.computer !== null) {
        others.push({
          node: device.name,
          name: device.name,
          os: probed.computer,
          addrs: [...device.addresses],
          online: true,
          lastSeenAt: device.lastSeenAt,
        });
      }
      continue;
    }
    if (os !== 'linux' && os !== 'macos' && os !== 'windows') {
      continue;
    }
    const online = apiDeviceOnline(device, now);
    if (os === 'linux' || device.tags.includes('tag:server')) {
      servers.push({
        node: device.name,
        addr: firstIpv4(device.addresses),
        online,
        os,
        tailscaleSsh: null,
      });
      continue;
    }
    others.push({
      node: device.name,
      name: device.name,
      os,
      addrs: [...device.addresses],
      online,
      lastSeenAt: device.lastSeenAt,
    });
  }
  return { servers, others };
}
