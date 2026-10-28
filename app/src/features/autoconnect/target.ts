import type { Directory, PairedHost } from '../../native/rscore/types';

export type AutoConnectTarget =
  | { kind: 'hostId'; hostId: string }
  | { kind: 'addr'; host: string; port?: number };

export type ResolvedTarget =
  | { hostId: string }
  | { hostId: string; ambiguous: true }
  | { error: 'unknownHost' };

const HOST_ID = /^[0-9a-z]{16}$/;
const HOST_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i;
const IPV4_PART = /^(?:0|[1-9]\d{0,2})$/;
const IPV6_PART = /^[0-9a-f]{1,4}$/i;

function isIpv4(value: string): boolean {
  const parts = value.split('.');
  return (
    parts.length === 4 &&
    parts.every(
      part => IPV4_PART.test(part) && Number(part) >= 0 && Number(part) <= 255,
    )
  );
}

function looksLikeIpv4(value: string): boolean {
  const parts = value.split('.');
  return parts.length === 4 && parts.every(part => /^\d+$/.test(part));
}

function isIpv6(value: string): boolean {
  if (!value.includes(':')) {
    return false;
  }
  const compression = value.indexOf('::');
  if (compression !== -1 && value.indexOf('::', compression + 2) !== -1) {
    return false;
  }

  const head = compression === -1 ? value : value.slice(0, compression);
  const tail = compression === -1 ? '' : value.slice(compression + 2);
  const parts = [
    ...(head === '' ? [] : head.split(':')),
    ...(tail === '' ? [] : tail.split(':')),
  ];
  if (parts.some(part => part === '')) {
    return false;
  }

  let groups = 0;
  for (const [index, part] of parts.entries()) {
    if (part.includes('.')) {
      if (index !== parts.length - 1 || !isIpv4(part)) {
        return false;
      }
      groups += 2;
    } else if (IPV6_PART.test(part)) {
      groups += 1;
    } else {
      return false;
    }
  }
  return compression === -1 ? groups === 8 : groups <= 7;
}

function isDnsName(value: string): boolean {
  if (value.length === 0 || value.length > 253 || looksLikeIpv4(value)) {
    return false;
  }
  return value.split('.').every(label => HOST_LABEL.test(label));
}

function parsePort(value: string): number | null {
  if (!/^\d{1,5}$/.test(value)) {
    return null;
  }
  const port = Number(value);
  return port >= 1 && port <= 65535 ? port : null;
}

export function parseAutoConnect(raw: string): AutoConnectTarget | null {
  if (raw === '' || /\s/.test(raw)) {
    return null;
  }
  if (HOST_ID.test(raw)) {
    return { kind: 'hostId', hostId: raw };
  }
  if (isIpv4(raw)) {
    return { kind: 'addr', host: raw };
  }

  const bracketed = /^\[([^\]]+)\]:(\d+)$/.exec(raw);
  if (bracketed !== null) {
    const host = bracketed[1] ?? '';
    const port = parsePort(bracketed[2] ?? '');
    return isIpv6(host) && port !== null ? { kind: 'addr', host, port } : null;
  }

  if (isIpv6(raw)) {
    return { kind: 'addr', host: raw };
  }

  const colon = raw.indexOf(':');
  if (colon !== -1) {
    if (raw.indexOf(':', colon + 1) !== -1) {
      return null;
    }
    const host = raw.slice(0, colon);
    const port = parsePort(raw.slice(colon + 1));
    if (port === null || (!isIpv4(host) && !isDnsName(host))) {
      return null;
    }
    return { kind: 'addr', host, port };
  }

  return isDnsName(raw) ? { kind: 'addr', host: raw } : null;
}

function addressKey(value: string): string {
  const text = value.trim();
  if (text.startsWith('[')) {
    const end = text.indexOf(']');
    return end === -1 ? text.toLowerCase() : text.slice(1, end).toLowerCase();
  }

  const colons = [...text].filter(char => char === ':').length;
  if (colons === 1) {
    const colon = text.indexOf(':');
    const host = text.slice(0, colon);
    const port = text.slice(colon + 1);
    if (/^\d+$/.test(port)) {
      return host.toLowerCase();
    }
  }
  return text.toLowerCase();
}

function match(
  hostId: string,
  addresses: readonly string[],
  address: string,
): boolean {
  return addresses.some(candidate => addressKey(candidate) === address);
}

function firstMatch(
  entries: readonly { hostId: string; addresses: readonly string[] }[],
  address: string,
): { hostId: string } | { hostId: string; ambiguous: true } | null {
  let hostId: string | null = null;
  let ambiguous = false;
  for (const entry of entries) {
    if (!match(entry.hostId, entry.addresses, address)) {
      continue;
    }
    if (hostId === null) {
      hostId = entry.hostId;
    } else if (hostId !== entry.hostId) {
      ambiguous = true;
    }
  }
  if (hostId === null) {
    return null;
  }
  return ambiguous ? { hostId, ambiguous: true } : { hostId };
}

export function resolveHostId(
  target: AutoConnectTarget,
  paired: readonly Pick<PairedHost, 'hostId' | 'addresses'>[],
  directory: Pick<Directory, 'computers'>,
): ResolvedTarget {
  if (target.kind === 'hostId') {
    return { hostId: target.hostId };
  }

  const address = addressKey(target.host);
  const pairedMatch = firstMatch(paired, address);
  if (pairedMatch !== null) {
    return pairedMatch;
  }
  const directoryMatch = firstMatch(
    directory.computers.map(computer => ({
      hostId: computer.hostId,
      addresses: computer.addresses.map(item => item.addr),
    })),
    address,
  );
  return directoryMatch ?? { error: 'unknownHost' };
}
