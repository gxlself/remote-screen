import type {
  CoreEventOf,
  Computer,
  DiscoverOutcome,
} from '../../native/rscore/types';
import { hostKind, parseHostPort } from '../ssh/serverForm';

export const RS_DEFAULT_PORT = 7400;
export const VNC_PORT = 5900;
export const RDP_PORT = 3389;

/**
 * A Remote Host answered on the RSP port. The real core only reports `hostId`
 * (derived from the TLS fingerprint); the other fields come from an optional
 * hello extension.
 */
export type ProbeHello = {
  hostId: string;
  name?: string;
  os?: 'macos' | 'windows';
  hostVersion?: string;
  fp?: string;
};

type UnknownRecord = Record<string, unknown>;

function isRecord(value: unknown): value is UnknownRecord {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value !== '';
}

function fullHelloOf(hello: unknown): ProbeHello | null {
  if (!isRecord(hello)) {
    return null;
  }
  if (
    !isNonEmptyString(hello.hostId) ||
    !isNonEmptyString(hello.name) ||
    !isNonEmptyString(hello.hostVersion) ||
    !isNonEmptyString(hello.fp) ||
    (hello.os !== 'macos' && hello.os !== 'windows')
  ) {
    return null;
  }
  return {
    hostId: hello.hostId,
    name: hello.name,
    os: hello.os,
    hostVersion: hello.hostVersion,
    fp: hello.fp,
  };
}

export function helloOf(e: CoreEventOf<'probe.result'>): ProbeHello | null {
  const full = fullHelloOf((e as { hello?: unknown }).hello);
  if (full !== null) {
    return full;
  }
  return isNonEmptyString(e.hostId) ? { hostId: e.hostId } : null;
}

export function parseAddressForm(
  host: string,
  port: string,
): { host: string; port: number } | { error: 'badHost' | 'badPort' } {
  let value = host.trim();
  // `host:port` and `[v6]:port` carry their own port; the port field wins.
  let embeddedPort: number | undefined;
  if (hostKind(value) === 'invalid') {
    const split = parseHostPort(value, RS_DEFAULT_PORT);
    if (split === null) {
      return { error: 'badHost' };
    }
    embeddedPort = /:\d+$/.test(value) ? split.port : undefined;
    value = split.host;
  }

  const portText = port.trim();
  if (portText === '') {
    return { host: value, port: embeddedPort ?? RS_DEFAULT_PORT };
  }
  if (!/^\d+$/.test(portText)) {
    return { error: 'badPort' };
  }
  const parsed = Number(portText);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
    return { error: 'badPort' };
  }
  return { host: value, port: parsed };
}

export type ComputerMatch = {
  computer: Computer;
  by: 'fp' | 'hostId' | 'address';
};

export function matchComputer(
  host: string,
  computers: Computer[],
  hello: ProbeHello | null,
): ComputerMatch | null {
  if (hello !== null) {
    const sameHost = computers.find(
      computer => computer.hostId === hello.hostId,
    );
    if (sameHost !== undefined) {
      // Without a hello fingerprint the hostId itself was derived from the
      // fingerprint, so a hostId match is a fingerprint match.
      return {
        computer: sameHost,
        by:
          hello.fp === undefined || sameHost.fp === hello.fp ? 'fp' : 'hostId',
      };
    }
  }

  const value = host.trim();
  const sameAddress = computers.find(computer =>
    computer.addresses.some(address => address.addr === value),
  );
  return sameAddress === undefined
    ? null
    : { computer: sameAddress, by: 'address' };
}

export type ProbeKind = 'rs' | 'rdp' | 'ssh';
export type NativeKind = 'vnc' | 'rdp';
/** `refused` (from `discover.probe`): the computer is up, the service is off. */
export type PortState = 'open' | 'refused' | 'closed';

export type AddrProbe = {
  state: 'idle' | 'running' | 'done';
  rs: CoreEventOf<'probe.result'> | null;
  hello: ProbeHello | null;
  rdp: PortState | null;
  /** Only probed with `core.discover`; stays null on older native packages. */
  vnc: PortState | null;
  ssh: 'open' | 'closed' | null;
  /** 5900 / 3389 / 22 went through `core.discover.probe` (V10). */
  discover: boolean;
  startedAtMs: number | null;
  elapsedMs: number | null;
};

export const IDLE_PROBE: AddrProbe = {
  state: 'idle',
  rs: null,
  hello: null,
  rdp: null,
  vnc: null,
  ssh: null,
  discover: false,
  startedAtMs: null,
  elapsedMs: null,
};

export function startProbe(nowMs: number, discover = false): AddrProbe {
  return {
    state: 'running',
    rs: null,
    hello: null,
    rdp: null,
    vnc: null,
    ssh: null,
    discover,
    startedAtMs: nowMs,
    elapsedMs: null,
  };
}

function portState(
  outcome: CoreEventOf<'probe.result'>['outcome'],
): 'open' | 'closed' {
  return outcome === 'ok' ? 'open' : 'closed';
}

function isComplete(s: AddrProbe): boolean {
  return (
    s.rs !== null &&
    s.rdp !== null &&
    s.ssh !== null &&
    (!s.discover || s.vnc !== null)
  );
}

/** Marks a running probe done once every expected port has a result. */
export function settleProbe(s: AddrProbe, nowMs: number): AddrProbe {
  if (s.state !== 'running' || !isComplete(s)) {
    return s;
  }
  return {
    ...s,
    state: 'done',
    elapsedMs:
      s.startedAtMs === null ? null : Math.max(0, nowMs - s.startedAtMs),
  };
}

export function applyProbe(
  s: AddrProbe,
  kind: ProbeKind,
  e: CoreEventOf<'probe.result'>,
  nowMs: number,
): AddrProbe {
  if (s.state !== 'running' || e.last !== true) {
    return s;
  }

  const next: AddrProbe = { ...s };
  if (kind === 'rs') {
    if (s.rs !== null) {
      return s;
    }
    next.rs = e;
    next.hello = helloOf(e);
  } else if (kind === 'rdp') {
    if (s.rdp !== null) {
      return s;
    }
    next.rdp = portState(e.outcome);
  } else {
    if (s.ssh !== null) {
      return s;
    }
    next.ssh = portState(e.outcome);
  }

  return settleProbe(next, nowMs);
}

export function nativePortState(outcome: DiscoverOutcome): PortState {
  switch (outcome) {
    case 'open':
      return 'open';
    case 'refused':
      return 'refused';
    case 'mismatch':
    case 'timeout':
      return 'closed';
  }
}

/** Applies a `discover.probe` result for 5900 (VNC), 3389 (RDP) or 22. */
export function applyDiscover(
  s: AddrProbe,
  kind: NativeKind | 'ssh',
  outcome: DiscoverOutcome,
  nowMs: number,
): AddrProbe {
  if (s.state !== 'running' || !s.discover || s[kind] !== null) {
    return s;
  }
  if (kind === 'ssh') {
    // A plain TCP connect: only `open` means sshd answered.
    return settleProbe(
      { ...s, ssh: outcome === 'open' ? 'open' : 'closed' },
      nowMs,
    );
  }
  const port = nativePortState(outcome);
  const next = kind === 'vnc' ? { ...s, vnc: port } : { ...s, rdp: port };
  return settleProbe(next, nowMs);
}

export function timeoutProbe(s: AddrProbe, nowMs: number): AddrProbe {
  if (s.state !== 'running') {
    return s;
  }
  return {
    ...s,
    state: 'done',
    rdp: s.rdp ?? 'closed',
    vnc: s.vnc ?? (s.discover ? 'closed' : null),
    ssh: s.ssh ?? 'closed',
    elapsedMs:
      s.startedAtMs === null ? null : Math.max(0, nowMs - s.startedAtMs),
  };
}

export type AddrVerdict = 'idle' | 'running' | 'known' | 'new' | 'notFound';

export function addrVerdict(
  s: AddrProbe,
  match: ComputerMatch | null,
): AddrVerdict {
  if (s.state === 'idle') {
    return 'idle';
  }
  if (s.state === 'running') {
    return 'running';
  }
  if (s.rs?.outcome !== 'ok') {
    return 'notFound';
  }
  return match === null ? 'new' : 'known';
}

export type NativeOs = 'macos' | 'windows';

export type NativeOffer = {
  os: NativeOs;
  protocol: NativeKind;
  port: number;
};

const NATIVE_OFFERS: NativeOffer[] = [
  { os: 'macos', protocol: 'vnc', port: VNC_PORT },
  { os: 'windows', protocol: 'rdp', port: RDP_PORT },
];

/** Screen Sharing first, then Remote Desktop; only from `discover` results. */
export function nativeOffers(s: AddrProbe): NativeOffer[] {
  if (s.state !== 'done' || !s.discover) {
    return [];
  }
  return NATIVE_OFFERS.filter(offer => s[offer.protocol] === 'open');
}

/**
 * Systems whose sharing service refused the connection. Nothing when another
 * native service is open; only the known OS when the Host told us which one.
 */
export function sharingOff(s: AddrProbe, os: NativeOs | null): NativeOs[] {
  if (s.state !== 'done' || !s.discover || nativeOffers(s).length > 0) {
    return [];
  }
  return NATIVE_OFFERS.filter(
    offer =>
      s[offer.protocol] === 'refused' && (os === null || offer.os === os),
  ).map(offer => offer.os);
}
