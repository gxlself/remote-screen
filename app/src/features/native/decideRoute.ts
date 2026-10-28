import type {
  AdapterProtocol,
  AdapterTarget,
  DiscoverOutcome,
} from '../../native/rscore/types';
import { BUILTIN_PROTOCOLS, targetForAddress } from '../adapters/logic';
import type { RemoteFallbackTarget } from '../fallback/useRemoteFallback';
import { pickAddress } from '../fallback/remoteFallback';
import { entryName, entryOs, entryProtocol } from './entries';
import type { ComputerEntry, ConnectVia } from './types';

type NativeOs = 'macos' | 'windows';
type AddrKind = 'tailnet' | 'intranet' | 'lan' | 'manual';

type AdapterRoute = {
  kind: 'adapter';
  target: AdapterTarget;
  name?: string;
  addrKind: AddrKind;
  context: 'native';
};

type SessionRoute = {
  kind: 'session';
  hostId: string;
  /** The native protocol failed after the host was known to be available. */
  fallback?: NativeOs;
};

type FallbackRoute = {
  kind: 'fallback';
  target: RemoteFallbackTarget;
};

type EnableSharingRoute = {
  kind: 'enableSharing';
  os: NativeOs;
  name: string;
  entryKey: string;
};

type ProbeRoute = {
  kind: 'probe';
  addr: string;
  port: number;
  protocol: AdapterProtocol;
  addrKind: Exclude<AddrKind, 'manual'>;
};

/** A navigation or side-effect decision made by the C7 rules. */
export type Route =
  | AdapterRoute
  | SessionRoute
  | FallbackRoute
  | EnableSharingRoute
  | ProbeRoute;

/**
 * Inputs used by the pure C7 decision function.
 *
 * `discoverAvailable` is optional so callers can omit it when they model the
 * current native package. Pass `false` explicitly for a legacy package whose
 * discover binding is absent; that is treated as the contract's "open" result.
 */
export type DecideRouteInput = {
  entry: ComputerEntry;
  adaptersAvailable?: boolean;
  adapters?: boolean;
  available?: boolean;
  builtinProtocols?: readonly AdapterProtocol[];
  connectPrefer?: 'native' | 'host';
  connectVia?: ConnectVia;
  via?: ConnectVia;
  discoverAvailable?: boolean;
  discover?: boolean;
  probe?: DiscoverOutcome | { outcome: DiscoverOutcome } | null;
  /** Optional aliases make the pure function convenient for small callers. */
  prefer?: 'native' | 'host';
  hostPrefs?: { connectVia?: ConnectVia };
  [key: string]: unknown;
};

const DEFAULT_PROTOCOLS: readonly AdapterProtocol[] = BUILTIN_PROTOCOLS;

function isIpv4(value: string): boolean {
  const octets = value.split('.');
  return (
    octets.length === 4 &&
    octets.every(octet => {
      if (!/^\d+$/.test(octet)) {
        return false;
      }
      const number = Number(octet);
      return number >= 0 && number <= 255;
    })
  );
}

function tailnetAddress(addrs: readonly string[]): string | null {
  return addrs.find(isIpv4) ?? addrs[0] ?? null;
}

function probeOutcome(
  probe: DecideRouteInput['probe'],
): DiscoverOutcome | null {
  if (probe === null || probe === undefined) {
    return null;
  }
  return typeof probe === 'string' ? probe : probe.outcome;
}

function nativeAdapter(
  protocol: AdapterProtocol,
  addr: string,
  addrKind: AddrKind,
  name: string | undefined,
  hostId?: string,
  port?: number,
): AdapterRoute | null {
  const target = targetForAddress(protocol, addr, port);
  if (target === null) {
    return null;
  }
  const withHost = hostId === undefined ? target : { ...target, hostId };
  return {
    kind: 'adapter',
    target: withHost,
    ...(name === undefined ? {} : { name }),
    addrKind,
    context: 'native',
  };
}

function fallbackRoute(
  os: NativeOs,
  addr: string | null,
  port?: number,
): FallbackRoute {
  return {
    kind: 'fallback',
    target: {
      os,
      addr: addr ?? '',
      ...(port === undefined ? {} : { port }),
    },
  };
}

/**
 * Decide the C7 connection route without reading React state or performing IO.
 * The hook calls this once before a probe and once with the probe result.
 */
export function decideRoute(input: DecideRouteInput): Route {
  const { entry } = input;
  const os = entryOs(entry);
  const protocol = entryProtocol(entry);
  const builtinProtocols = input.builtinProtocols ?? DEFAULT_PROTOCOLS;
  const adaptersAvailable =
    input.adaptersAvailable ?? input.adapters ?? input.available ?? false;
  const protocolAvailable = builtinProtocols.includes(protocol);

  // A missing adapter binding is the old package. Host entries retain the
  // existing Host route; host-less entries retain the external-app fallback.
  if (!adaptersAvailable || !protocolAvailable) {
    if (entry.kind === 'host') {
      return { kind: 'session', hostId: entry.computer.hostId };
    }
    if (entry.kind === 'tailnet') {
      return fallbackRoute(os, tailnetAddress(entry.device.addrs));
    }
    return fallbackRoute(os, entry.manual.host, entry.manual.port);
  }

  if (entry.kind === 'tailnet') {
    const addr = tailnetAddress(entry.device.addrs);
    if (addr === null) {
      return fallbackRoute(os, addr);
    }
    return (
      nativeAdapter(protocol, addr, 'tailnet', entry.device.name) ??
      fallbackRoute(os, addr)
    );
  }

  if (entry.kind === 'manual') {
    return (
      nativeAdapter(
        protocol,
        entry.manual.host,
        'manual',
        entry.manual.name ?? entry.manual.host,
        undefined,
        entry.manual.port,
      ) ?? fallbackRoute(os, entry.manual.host, entry.manual.port)
    );
  }

  const hostPrefs = input.hostPrefs;
  const selectedVia =
    input.connectVia ??
    input.via ??
    hostPrefs?.connectVia ??
    input.connectPrefer ??
    input.prefer ??
    'native';
  const via =
    selectedVia === 'auto'
      ? input.connectPrefer ?? input.prefer ?? 'native'
      : selectedVia;
  if (via === 'host') {
    return { kind: 'session', hostId: entry.computer.hostId };
  }

  const addr = pickAddress(entry.computer.addresses);
  const address =
    addr === null
      ? undefined
      : entry.computer.addresses.find(candidate => candidate.addr === addr);
  const result = probeOutcome(input.probe);

  // Legacy native packages have no discover namespace. C7 treats that as an
  // optimistic open result so the AdapterSession can report its own error.
  const discoverAvailable = input.discoverAvailable ?? input.discover ?? true;
  if (result === null && discoverAvailable !== true) {
    return addr === null || address === undefined
      ? { kind: 'session', hostId: entry.computer.hostId }
      : nativeAdapter(
          protocol,
          addr,
          address.kind,
          entry.computer.name,
          entry.computer.hostId,
        ) ?? { kind: 'session', hostId: entry.computer.hostId };
  }

  if (addr === null || address === undefined) {
    return { kind: 'session', hostId: entry.computer.hostId };
  }

  if (result === null) {
    return {
      kind: 'probe',
      addr,
      port: protocol === 'vnc' ? 5900 : 3389,
      protocol,
      addrKind: address.kind,
    };
  }

  if (result === 'open') {
    return (
      nativeAdapter(
        protocol,
        addr,
        address.kind,
        entry.computer.name,
        entry.computer.hostId,
      ) ?? { kind: 'session', hostId: entry.computer.hostId }
    );
  }

  if (entry.computer.online && entry.computer.hostRunning) {
    return {
      kind: 'session',
      hostId: entry.computer.hostId,
      fallback: os,
    };
  }

  if (result === 'refused') {
    return {
      kind: 'enableSharing',
      os,
      name: entryName(entry),
      entryKey: entry.key,
    };
  }

  return { kind: 'session', hostId: entry.computer.hostId };
}
