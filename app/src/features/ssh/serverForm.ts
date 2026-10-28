import type {
  CoreEventOf,
  DiscoverProbe,
  ProbeOutcome,
  SshAuth,
  SshServer,
} from '../../native/rscore/types';
import { ipInCidr, isTailnetAddr, parseIpv4 } from '../network/cidr';
import type { NetworkEntry } from '../settings/schema';
import type { ManualServer } from '../servers/manual';

export type ServerForm = {
  name: string;
  user: string;
  host: string;
  port: string;
  auth: SshAuth;
  jumpOn: boolean;
  jump: string;
};

export const EMPTY_FORM: ServerForm = {
  name: '',
  user: '',
  host: '',
  port: '22',
  auth: 'deviceKey',
  jumpOn: false,
  jump: '',
};

export type FieldKey = 'name' | 'user' | 'host' | 'port' | 'jump' | 'auth';

export type FormErrorCode =
  | 'required'
  | 'badHost'
  | 'badPort'
  | 'duplicateName'
  | 'tailscaleNeedsTailnet'
  | 'noKey';

/** Visual order on the form; the first invalid one gets focus on save. */
export const FIELD_ORDER: FieldKey[] = [
  'name',
  'user',
  'host',
  'port',
  'auth',
  'jump',
];

const PRIVATE_CIDRS = ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16'];
const HOST_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
/** Tailscale hands out IPv6 addresses from this ULA prefix. */
const TAILNET_V6_PREFIX = 'fd7a:115c:a1e0:';

function parsePortText(value: string): number | null {
  const text = value.trim();
  if (!/^\d+$/.test(text)) {
    return null;
  }

  const port = Number(text);
  return Number.isInteger(port) && port >= 1 && port <= 65535 ? port : null;
}

function isValidPort(port: number): boolean {
  return Number.isInteger(port) && port >= 1 && port <= 65535;
}

function isHostname(value: string): boolean {
  return value.split('.').every(label => HOST_LABEL.test(label));
}

/** A bare IPv6 literal, such as `fd7a:115c:a1e0::3` or `::1`. */
export function isIpv6(value: string): boolean {
  if (!/^[0-9a-f:]+$/i.test(value) || !/^[^:]*(:[^:]*){2,7}$/.test(value)) {
    return false;
  }
  const compressed = value.split('::').length - 1;
  if (compressed > 1 || (compressed === 0 && value.split(':').length !== 8)) {
    return false;
  }
  return value
    .split(':')
    .every(group => group === '' || /^[0-9a-f]{1,4}$/i.test(group));
}

export type HostKind =
  | 'tailnet'
  | 'magicDns'
  | 'private'
  | 'public'
  | 'name'
  | 'invalid';

export function hostKind(host: string): HostKind {
  const value = host.trim();
  if (value === '' || /\s/.test(value)) {
    return 'invalid';
  }

  if (parseIpv4(value) !== null) {
    if (isTailnetAddr(value)) {
      return 'tailnet';
    }
    if (PRIVATE_CIDRS.some(cidr => ipInCidr(value, cidr))) {
      return 'private';
    }
    return 'public';
  }

  if (isIpv6(value)) {
    const lower = value.toLowerCase();
    if (lower.startsWith(TAILNET_V6_PREFIX)) {
      return 'tailnet';
    }
    // Unique-local fc00::/7 and link-local fe80::/10.
    return /^f[cd]/.test(lower) || /^fe[89ab]/.test(lower)
      ? 'private'
      : 'public';
  }

  const labels = value.toLowerCase().split('.');
  if (labels.length === 4 && labels.every(label => /^\d+$/.test(label))) {
    return 'invalid';
  }
  if (!isHostname(value.toLowerCase())) {
    return 'invalid';
  }

  return labels.length === 1 || value.toLowerCase().endsWith('.ts.net')
    ? 'magicDns'
    : 'name';
}

export function validateServerForm(
  f: ServerForm,
  ctx: {
    existingNames: string[];
    editingName?: string;
    /** False when this phone has no SSH key yet; unknown when omitted. */
    hasKey?: boolean;
    /** Saved servers the jump field may name instead of an address. */
    jumpNames?: string[];
  },
): Partial<Record<FieldKey, FormErrorCode>> {
  const errors: Partial<Record<FieldKey, FormErrorCode>> = {};
  const host = f.host.trim();
  const kind = hostKind(host);

  if (host === '') {
    errors.host = 'required';
  } else if (kind === 'invalid') {
    errors.host = 'badHost';
  }

  if (parsePortText(f.port) === null) {
    errors.port = 'badPort';
  }

  if (f.user.trim() === '') {
    errors.user = 'required';
  }

  const name = f.name.trim().toLowerCase();
  if (name !== '') {
    const editingName = ctx.editingName?.trim().toLowerCase();
    const duplicate = ctx.existingNames.some(
      existing =>
        existing.trim().toLowerCase() !== editingName &&
        existing.trim().toLowerCase() === name,
    );
    if (duplicate) {
      errors.name = 'duplicateName';
    }
  }

  if (f.auth === 'tailscale' && kind !== 'tailnet' && kind !== 'magicDns') {
    errors.auth = 'tailscaleNeedsTailnet';
  } else if (f.auth === 'tailscale' && f.jumpOn) {
    errors.auth = 'tailscaleNeedsTailnet';
  } else if (f.auth === 'deviceKey' && ctx.hasKey === false) {
    errors.auth = 'noKey';
  }

  if (f.jumpOn && f.jump.trim() === '') {
    errors.jump = 'required';
  } else if (
    f.jumpOn &&
    parseHostPort(f.jump, 22) === null &&
    !(ctx.jumpNames ?? []).includes(f.jump.trim())
  ) {
    errors.jump = 'badHost';
  }

  return errors;
}

export function requiredNetwork(
  host: string,
  networks: NetworkEntry[],
): NetworkEntry | null {
  const value = host.trim();
  if (parseIpv4(value) === null) {
    return null;
  }

  return (
    networks.find(network =>
      network.cidrs.some(cidr => ipInCidr(value, cidr)),
    ) ?? null
  );
}

export function shouldSuggestJump(
  f: ServerForm,
  networks: NetworkEntry[],
): boolean {
  return !f.jumpOn && requiredNetwork(f.host, networks) !== null;
}

export function parseHostPort(
  s: string,
  defPort: number,
): { host: string; port: number } | null {
  const value = s.trim();
  if (value === '') {
    return null;
  }

  // IPv6 takes its port only in brackets: `[fd7a::3]:22`.
  const bracketed = /^\[([^\]]+)\](?::(\d+))?$/.exec(value);
  if (bracketed !== null) {
    const port =
      bracketed[2] === undefined ? defPort : parsePortText(bracketed[2]);
    return isIpv6(bracketed[1]) && port !== null && isValidPort(port)
      ? { host: bracketed[1], port }
      : null;
  }
  if (isIpv6(value)) {
    return isValidPort(defPort) ? { host: value, port: defPort } : null;
  }

  const colon = value.indexOf(':');
  if (colon === -1) {
    if (hostKind(value) === 'invalid' || !isValidPort(defPort)) {
      return null;
    }
    return { host: value, port: defPort };
  }
  if (value.indexOf(':', colon + 1) !== -1) {
    return null;
  }

  const host = value.slice(0, colon).trim();
  const port = parsePortText(value.slice(colon + 1));
  if (host === '' || port === null || hostKind(host) === 'invalid') {
    return null;
  }
  return { host, port };
}

export function formToServer(
  f: ServerForm,
  ctx: {
    id: string;
    nowUs: number;
    addedAtUs?: number;
    networks: NetworkEntry[];
  },
): ManualServer {
  const host = f.host.trim();
  const name = f.name.trim();
  return {
    id: ctx.id,
    kind: 'ssh',
    name: name === '' ? host : name,
    host,
    port: Number(f.port.trim()),
    user: f.user.trim(),
    auth: f.auth,
    jump: f.jumpOn ? f.jump.trim() : null,
    networkId: requiredNetwork(host, ctx.networks)?.id ?? null,
    addedAtUs: ctx.addedAtUs ?? ctx.nowUs,
    updatedAtUs: ctx.nowUs,
  };
}

export function serverToForm(m: ManualServer | SshServer): ServerForm {
  return {
    name: m.name,
    user: m.user,
    host: m.host,
    port: String(m.port),
    auth: m.auth,
    jumpOn: m.jump !== null && m.jump !== '',
    jump: m.jump ?? '',
  };
}

export type TestState =
  | { kind: 'idle' }
  | { kind: 'running'; probeId: string | null }
  | { kind: 'ok'; ms: number | null; via: string | null }
  | {
      kind: 'fail';
      reason: ProbeOutcome | 'unsupported' | 'noResult';
      via: string | null;
    };

export function testTarget(
  f: ServerForm,
): { host: string; port: number; via: string | null } | null {
  if (Object.keys(validateServerForm(f, { existingNames: [] })).length > 0) {
    return null;
  }

  const host = f.host.trim();
  const port = Number(f.port.trim());

  if (f.jumpOn) {
    const jump = parseHostPort(f.jump, 22);
    return jump === null
      ? null
      : { host: jump.host, port: jump.port, via: f.jump.trim() };
  }

  return { host, port, via: null };
}

export function testFromProbe(
  e: CoreEventOf<'probe.result'>,
  via: string | null,
): TestState {
  return e.outcome === 'ok'
    ? { kind: 'ok', ms: e.ms, via }
    : { kind: 'fail', reason: e.outcome, via };
}

/** Maps a V10 `discover.probe` TCP connect to the SSH port. */
export function testFromDiscover(
  p: DiscoverProbe,
  via: string | null,
): TestState {
  switch (p.outcome) {
    case 'open':
      return { kind: 'ok', ms: p.ms, via };
    case 'refused':
    case 'timeout':
      return { kind: 'fail', reason: p.outcome, via };
    case 'mismatch':
      // Never reported for 'tcp'; treated like a closed port.
      return { kind: 'fail', reason: 'refused', via };
  }
}
