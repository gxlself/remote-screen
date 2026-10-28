import { useCallback, useMemo } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { DiscoverKind, TailscaleDevice } from '../../native/rscore/types';
import { parseIpv4 } from '../network/cidr';
import { useKv } from '../storage/StorageContext';
import type { KvStore } from '../storage/kv';
import type { ProbeClass, TailnetProbes } from './apiDevices';

/** Port results by device id: `{ addr, at, vnc, rdp, ssh }`, `at` in ms. */
export const TAILNET_PROBE_KEY = 'tailnet.probe.v1';
/** A device is probed again after this long, or on a forced refresh. */
export const PROBE_TTL_MS = 10 * 60_000;
export const PROBE_TIMEOUT_MS = 1_500;
/** One-off directory reads wait at most this long for a probe round. */
export const PROBE_WAIT_MS = 2_000;
/** How often the hook looks for devices that are due. */
export const PROBE_CHECK_MS = 60_000;
/** Devices probed at once, three ports each. */
const PROBE_CONCURRENCY = 4;

type Discover = NonNullable<RsCoreApi['discover']>;
type Ports = { vnc: boolean; rdp: boolean; ssh: boolean };
type Entry = Ports & { addr: string; at: number };

export type ProbeTarget = { id: string; addr: string };

/** Devices without a system that are not known to be offline, at their first IPv4. */
export function probeTargets(
  devices: readonly TailscaleDevice[],
): ProbeTarget[] {
  const targets: ProbeTarget[] = [];
  for (const device of devices) {
    const addr = device.addresses.find(ip => parseIpv4(ip) !== null);
    if (
      device.os === 'unknown' &&
      device.online !== false &&
      addr !== undefined
    ) {
      targets.push({ id: device.id, addr });
    }
  }
  return targets;
}

/**
 * 3389 makes a Windows PC even with 5900 open too (a VNC server on Windows;
 * a Mac never listens on 3389), else 5900 a Mac; 22 adds an SSH server.
 */
export function classifyPorts(ports: Ports): ProbeClass {
  return {
    computer: ports.rdp ? 'windows' : ports.vnc ? 'macos' : null,
    ssh: ports.ssh,
  };
}

function readEntries(kv: KvStore): Record<string, Entry> {
  const stored = kv.getJSON<Record<string, unknown>>(TAILNET_PROBE_KEY);
  const entries: Record<string, Entry> = {};
  if (stored == null || typeof stored !== 'object' || Array.isArray(stored)) {
    return entries;
  }
  for (const [id, value] of Object.entries(stored)) {
    const entry = value as Partial<Record<keyof Entry, unknown>> | null;
    if (
      typeof entry?.addr === 'string' &&
      typeof entry.at === 'number' &&
      Number.isFinite(entry.at) &&
      typeof entry.vnc === 'boolean' &&
      typeof entry.rdp === 'boolean' &&
      typeof entry.ssh === 'boolean'
    ) {
      entries[id] = {
        addr: entry.addr,
        at: entry.at,
        vnc: entry.vnc,
        rdp: entry.rdp,
        ssh: entry.ssh,
      };
    }
  }
  return entries;
}

/** Saved classes however old; a device at another address has none. */
export function readTailnetProbes(
  kv: KvStore,
  targets: readonly ProbeTarget[],
): TailnetProbes {
  const entries = readEntries(kv);
  const probes: TailnetProbes = {};
  for (const target of targets) {
    const entry = entries[target.id];
    if (entry !== undefined && entry.addr === target.addr) {
      probes[target.id] = classifyPorts(entry);
    }
  }
  return probes;
}

/** 5900, 3389 and 22 at once; anything but `open` (or a failed call) is closed. */
async function probePorts(discover: Discover, addr: string): Promise<Ports> {
  const open = async (port: number, kind: DiscoverKind) => {
    try {
      const result = await discover.probe(addr, port, kind, PROBE_TIMEOUT_MS);
      return result.outcome === 'open';
    } catch {
      return false;
    }
  };
  const [vnc, rdp, ssh] = await Promise.all([
    open(5900, 'vnc'),
    open(3389, 'rdp'),
    open(22, 'tcp'),
  ]);
  return { vnc, rdp, ssh };
}

/**
 * Probe the targets whose saved result is missing, for another address or
 * older than PROBE_TTL_MS (every target when `force`), save the results and
 * resolve with the classes of all targets. Saved results of devices that are
 * no longer targets are dropped.
 */
export async function probeTailnet(
  core: RsCoreApi,
  kv: KvStore,
  targets: readonly ProbeTarget[],
  opts: { force?: boolean; now?: () => number } = {},
): Promise<TailnetProbes> {
  const discover = core.discover;
  const now = opts.now ?? Date.now;
  const saved = readEntries(kv);
  const due = targets.filter(target => {
    const entry = saved[target.id];
    return (
      opts.force === true ||
      entry === undefined ||
      entry.addr !== target.addr ||
      now() - entry.at >= PROBE_TTL_MS
    );
  });
  if (discover === undefined || due.length === 0) {
    return readTailnetProbes(kv, targets);
  }
  const results: Record<string, Entry> = {};
  let next = 0;
  const worker = async () => {
    while (next < due.length) {
      const target = due[next];
      next += 1;
      if (target === undefined) {
        continue;
      }
      const ports = await probePorts(discover, target.addr);
      results[target.id] = { ...ports, addr: target.addr, at: now() };
    }
  };
  await Promise.all(
    Array.from({ length: Math.min(PROBE_CONCURRENCY, due.length) }, worker),
  );
  const latest = readEntries(kv);
  const kept: Record<string, Entry> = {};
  for (const target of targets) {
    const entry = results[target.id] ?? latest[target.id];
    if (entry !== undefined) {
      kept[target.id] = entry;
    }
  }
  kv.setJSON(TAILNET_PROBE_KEY, kept);
  return readTailnetProbes(kv, targets);
}

/**
 * For one-off reads: the classes once a probe round ends or PROBE_WAIT_MS
 * passes, whichever comes first. A slower round still saves its results.
 */
export async function tailnetProbesWithin(
  core: RsCoreApi,
  kv: KvStore,
  targets: readonly ProbeTarget[],
  waitMs = PROBE_WAIT_MS,
): Promise<TailnetProbes> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const waited = new Promise<null>(resolve => {
    timer = setTimeout(() => resolve(null), waitMs);
  });
  const round = probeTailnet(core, kv, targets).catch(() => null);
  try {
    return (
      (await Promise.race([round, waited])) ?? readTailnetProbes(kv, targets)
    );
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Classes of the devices without a system: the saved ones at once, then each
 * round's. While `enabled` a round runs every PROBE_CHECK_MS and probes only
 * the devices that are due; `refresh` probes all of them again.
 */
export function useTailnetProbes(
  devices: readonly TailscaleDevice[] | null,
  enabled: boolean,
): { probes: TailnetProbes; refresh: () => Promise<unknown> } {
  const core = useCore();
  const kv = useKv();
  const queryClient = useQueryClient();
  const signature = JSON.stringify(
    probeTargets(devices ?? []).sort((a, b) => a.id.localeCompare(b.id)),
  );
  const targets = useMemo(
    () => JSON.parse(signature) as ProbeTarget[],
    [signature],
  );
  const cached = useMemo(() => readTailnetProbes(kv, targets), [kv, targets]);
  const queryKey = useMemo(() => ['tailnetProbe', signature], [signature]);
  const active = enabled && targets.length > 0;
  const query = useQuery({
    queryKey,
    queryFn: () => probeTailnet(core, kv, targets),
    enabled: active,
    staleTime: PROBE_CHECK_MS,
    refetchInterval: PROBE_CHECK_MS,
    refetchIntervalInBackground: false,
    retry: false,
  });
  const refresh = useCallback(async () => {
    if (!active) {
      return;
    }
    const probes = await probeTailnet(core, kv, targets, { force: true });
    queryClient.setQueryData(queryKey, probes);
  }, [active, core, kv, queryClient, queryKey, targets]);
  return { probes: query.data ?? cached, refresh };
}
