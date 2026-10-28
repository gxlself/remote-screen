import type {
  Computer,
  CoreEventOf,
  NetPath,
  ProbeOutcome,
  TailnetStatus,
  Transport,
} from '../../native/rscore/types';
import { derpName, peerForAddr, peerPath } from '../tailnet/peers';

export type ProbeRow = {
  key: string;
  path: NetPath;
  transport: Transport;
  addr: string;
  port: number;
  state: 'pending' | 'ok' | 'fail';
  outcome: ProbeOutcome | null;
  ms: number | null;
  reason: string | null;
};

export function rowKey(
  path: NetPath,
  addr: string,
  transport: Transport,
): string {
  return `${path}|${addr}|${transport}`;
}

function row(
  path: NetPath,
  transport: Transport,
  addr: string,
  port: number,
): ProbeRow {
  return {
    key: rowKey(path, addr, transport),
    path,
    transport,
    addr,
    port,
    state: 'pending',
    outcome: null,
    ms: null,
    reason: null,
  };
}

export function expectedRows(c: Computer): ProbeRow[] {
  return c.addresses.flatMap(address => {
    switch (address.kind) {
      case 'tailnet':
        return [
          row('tailnetDirect', 'quic', address.addr, address.port),
          row('tailnetDerp', 'quic', address.addr, address.port),
        ];
      case 'intranet':
      case 'lan':
        return [
          row(address.kind, 'quic', address.addr, address.port),
          row(address.kind, 'tcp', address.addr, address.port),
        ];
    }
  });
}

export function applyProbeResult(
  rows: ProbeRow[],
  e: CoreEventOf<'probe.result'>,
): ProbeRow[] {
  const key = rowKey(e.path, e.addr, e.transport);
  const index = rows.findIndex(item => item.key === key);
  const next: ProbeRow = {
    key,
    path: e.path,
    transport: e.transport,
    addr: e.addr,
    port: e.port,
    state: e.outcome === 'ok' ? 'ok' : 'fail',
    outcome: e.outcome,
    ms: e.ms,
    reason: e.reason ?? null,
  };

  if (index === -1) {
    return [...rows, next];
  }
  return rows.map((item, itemIndex) => (itemIndex === index ? next : item));
}

export function finishRows(rows: ProbeRow[]): ProbeRow[] {
  return rows.map(item =>
    item.state === 'pending'
      ? {
          ...item,
          state: 'fail',
          outcome: 'timeout',
          ms: null,
          reason: null,
        }
      : item,
  );
}

/** App 内连接生效时，按对端实况把 tailnet 结果摆到直连 / 中继行。不改入参。 */
export function applyEmbeddedPaths(
  rows: ProbeRow[],
  status: TailnetStatus | null,
): ProbeRow[] {
  if (status === null) {
    return rows;
  }

  const replacements = new Map<string, ProbeRow>();
  for (const item of rows) {
    if (item.path !== 'tailnetDirect') {
      continue;
    }
    const directKey = rowKey('tailnetDirect', item.addr, 'quic');
    const derpKey = rowKey('tailnetDerp', item.addr, 'quic');
    const direct = rows.find(candidate => candidate.key === directKey);
    const derp = rows.find(candidate => candidate.key === derpKey);
    if (direct === undefined || derp === undefined) {
      continue;
    }

    const kind = peerPath(peerForAddr(status, item.addr));
    if (kind === 'relay' && direct.state === 'ok') {
      replacements.set(direct.key, {
        ...direct,
        state: 'fail',
        outcome: 'timeout',
        ms: null,
        reason: 'holePunchFailed',
      });
      replacements.set(derp.key, {
        ...derp,
        state: 'ok',
        outcome: 'ok',
        ms: direct.ms,
        reason: null,
      });
    } else if (
      kind === 'direct' &&
      direct.state === 'ok' &&
      derp.state !== 'ok'
    ) {
      replacements.set(derp.key, {
        ...derp,
        state: 'fail',
        outcome: 'skipped',
        ms: null,
        reason: 'skipped',
      });
    }
  }

  if (replacements.size === 0) {
    return rows;
  }
  return rows.map(item => replacements.get(item.key) ?? item);
}

/** tailnet 地址 → DERP 地域名（没有对端或对端没有 relay 时不出现在结果里） */
export function derpNamesFor(
  rows: ProbeRow[],
  status: TailnetStatus | null,
): Record<string, string> {
  const result: Record<string, string> = {};
  if (status === null) {
    return result;
  }
  for (const item of rows) {
    if (item.path !== 'tailnetDirect') {
      continue;
    }
    const peer = peerForAddr(status, item.addr);
    if (peer !== null && peer.relay !== '') {
      result[item.addr] = derpName(status, peer.relay);
    }
  }
  return result;
}

export type Advice =
  | { kind: 'running' }
  | { kind: 'best'; row: ProbeRow; tcpMode: boolean }
  | { kind: 'allFailed' };

export function advice(rows: ProbeRow[], done: boolean): Advice {
  if (!done) {
    return { kind: 'running' };
  }

  let best: ProbeRow | null = null;
  for (const item of rows) {
    if (item.state !== 'ok') {
      continue;
    }
    if (
      best === null ||
      (item.ms ?? Number.POSITIVE_INFINITY) <
        (best.ms ?? Number.POSITIVE_INFINITY)
    ) {
      best = item;
    }
  }

  if (best === null) {
    return { kind: 'allFailed' };
  }

  const tcpMode =
    best.transport === 'tcp' &&
    rows.some(
      item =>
        item.path === best?.path &&
        item.transport === 'quic' &&
        item.state === 'fail',
    );

  return { kind: 'best', row: best, tcpMode };
}
