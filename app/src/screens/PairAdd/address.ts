import { parseAutoConnect } from '../../features/autoconnect/target';

export type PairAddress = { addr: string; port?: number };

/**
 * Accepts IPv4, IPv6 (bare or `[v6]` / `[v6]:port`) and host names, each with
 * an optional `:port`. Whitespace is ignored anywhere in the input.
 */
export function parsePairAddress(text: string): PairAddress | null {
  const value = text.replace(/\s+/g, '');
  const bracketed = /^\[([^\]]+)\]$/.exec(value)?.[1];
  const target = parseAutoConnect(bracketed ?? value);
  if (target === null) {
    return null;
  }
  // A 16-character host id is also a valid single-label host name.
  const addr = target.kind === 'hostId' ? target.hostId : target.host;
  if (bracketed !== undefined && !addr.includes(':')) {
    return null;
  }
  return target.kind === 'addr' && target.port !== undefined
    ? { addr, port: target.port }
    : { addr };
}

export function formatPairAddress(addr?: string, port?: number): string {
  if (addr === undefined) {
    return '';
  }
  if (port === undefined) {
    return addr;
  }
  return addr.includes(':') ? `[${addr}]:${port}` : `${addr}:${port}`;
}
