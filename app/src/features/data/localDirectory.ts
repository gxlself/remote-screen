import type {
  Address,
  Computer,
  Directory,
  Micros,
  OtherDevice,
  PairedHost,
  Server,
  TailnetPeer,
  TailnetStatus,
  TailscaleDevice,
} from '../../native/rscore/types';
import { RSP_DEFAULT_PORT } from '../../native/rscore/RsCoreApi';
import { isTailnetAddr, parseIpv4 } from '../network/cidr';
import { apiDirectoryEntries } from '../tailscale/apiDevices';
import type { TailnetProbes } from '../tailscale/apiDevices';

export type LocalReach = Record<string, { online: boolean; at: Micros }>;

type DirectoryOs = Computer['os'];

/** "ip:port" or "[v6]:port", as Rust stores SocketAddr. */
function splitAddress(value: string): { addr: string; port: number } | null {
  let addr = value;
  let port = '';
  if (value.startsWith('[')) {
    const end = value.indexOf(']');
    if (end === -1) {
      return null;
    }
    addr = value.slice(1, end);
    port = value.slice(end + 1).replace(/^:/, '');
  } else if (value.split(':').length === 2) {
    [addr, port] = value.split(':') as [string, string];
  }
  if (addr === '') {
    return null;
  }
  const parsed = /^\d{1,5}$/.test(port) ? Number(port) : 0;
  return {
    addr,
    port: parsed >= 1 && parsed <= 65535 ? parsed : RSP_DEFAULT_PORT,
  };
}

function toAddress(value: string): Address | null {
  const split = splitAddress(value);
  if (split === null) {
    return null;
  }
  return {
    kind: isTailnetAddr(split.addr) ? 'tailnet' : 'lan',
    addr: split.addr,
    port: split.port,
  };
}

/** Normalize the operating system reported by a tailnet node. */
function nativeOs(os: string): DirectoryOs | null {
  const value = os.toLowerCase();
  if (value.includes('mac') || value === 'darwin') {
    return 'macos';
  }
  if (value.startsWith('windows')) {
    return 'windows';
  }
  return null;
}

/** Linux Hosts are shown as Windows for now. */
function hostOs(os: string): DirectoryOs | null {
  return os.toLowerCase() === 'linux' ? 'windows' : nativeOs(os);
}

function hasTag(peer: TailnetPeer, tag: string): boolean {
  return peer.tags.includes(tag);
}

function buildComputer(
  host: PairedHost,
  reach: LocalReach,
  peers: TailnetPeer[],
): Computer {
  const addresses = host.addresses
    .map(toAddress)
    .filter((address): address is Address => address !== null);
  const ips = new Set(addresses.map(address => address.addr));
  const peer = peers.find(candidate => candidate.ips.some(ip => ips.has(ip)));
  const seen = reach[host.hostId];
  const online = seen?.online ?? false;
  return {
    hostId: host.hostId,
    name: host.name,
    node: peer?.hostname ?? '',
    os:
      (host.os === null ? null : hostOs(host.os)) ??
      (peer === undefined ? null : hostOs(peer.os)) ??
      'macos',
    ...(host.osVersion === null ? {} : { osVersion: host.osVersion }),
    online,
    hostRunning: online,
    lastSeenAt: Math.max(seen?.at ?? 0, host.lastSeenAt ?? 0, host.pairedAt),
    lastHeartbeatAt: null,
    hostVersion: host.hostVersion ?? '',
    features: host.features ?? 0,
    fp: host.fp,
    addresses,
    displays: host.displays.map(display => ({ ...display })),
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: '',
    filevault: false,
    console: null,
    wake: null,
  };
}

function buildServer(peer: TailnetPeer): Server {
  return {
    node: peer.hostname,
    addr: peer.ips.find(ip => parseIpv4(ip) !== null) ?? peer.ips[0] ?? '',
    online: peer.online,
    os: peer.os,
    tailscaleSsh: null,
  };
}

export function buildLocalDirectory(input: {
  paired: PairedHost[];
  reach: LocalReach;
  /** Only the in-app tailnet lists nodes; system Tailscale passes null. */
  tailnet: TailnetStatus | null;
  /** From the Tailscale API token; null when it is not set up or failed. */
  apiDevices?: TailscaleDevice[] | null;
  /** Port classes of the API devices without a system (Headscale). */
  apiProbes?: TailnetProbes;
  now: Micros;
}): Directory {
  const peers = input.tailnet?.peers ?? [];
  const computers = input.paired.map(host =>
    buildComputer(host, input.reach, peers),
  );
  const pairedIps = new Set(
    computers.flatMap(computer =>
      computer.addresses.map(address => address.addr),
    ),
  );
  const serverPeers = peers.filter(peer => hasTag(peer, 'tag:server'));
  const servers = serverPeers.map(buildServer);
  // API devices are dropped when a Host or a listed peer already has the address.
  const listedIps = new Set([
    ...pairedIps,
    ...serverPeers.flatMap(peer => peer.ips),
  ]);
  const others: OtherDevice[] = [];
  for (const peer of peers) {
    const os = nativeOs(peer.os);
    if (
      os === null ||
      hasTag(peer, 'tag:rs-host') ||
      hasTag(peer, 'tag:server') ||
      peer.ips.some(ip => pairedIps.has(ip))
    ) {
      continue;
    }
    others.push({
      node: peer.hostname,
      name: peer.hostname,
      os,
      addrs: [...peer.ips],
      online: peer.online,
      lastSeenAt: null,
    });
    for (const ip of peer.ips) {
      listedIps.add(ip);
    }
  }
  const api = apiDirectoryEntries(
    input.apiDevices ?? [],
    listedIps,
    input.now,
    input.apiProbes,
  );
  return {
    generatedAt: input.now,
    computers,
    servers: [...servers, ...api.servers],
    others: [...others, ...api.others],
  };
}
