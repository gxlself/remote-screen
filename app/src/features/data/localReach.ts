import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { CoreEventOf, Micros } from '../../native/rscore/types';
import { useKv } from '../storage/StorageContext';
import type { KvStore } from '../storage/kv';
import type { LocalReach } from './localDirectory';

export const LOCAL_REACH_KEY = 'cache.localReach';
export const REACH_INTERVAL_MS = 30_000;
export const REACH_CONCURRENCY = 8;
export const REACH_TIMEOUT_MS = 1_500;

type ProbeResult = CoreEventOf<'probe.result'>;

function nowUs(): Micros {
  return Date.now() * 1000;
}

export function readLocalReach(kv: KvStore): LocalReach {
  const stored = kv.getJSON<Record<string, unknown>>(LOCAL_REACH_KEY);
  const reach: LocalReach = {};
  if (stored === null || typeof stored !== 'object' || Array.isArray(stored)) {
    return reach;
  }
  for (const [hostId, value] of Object.entries(stored)) {
    const entry = value as { online?: unknown; at?: unknown } | null;
    if (
      typeof entry?.online === 'boolean' &&
      typeof entry.at === 'number' &&
      Number.isFinite(entry.at)
    ) {
      reach[hostId] = { online: entry.online, at: entry.at };
    }
  }
  return reach;
}

async function probeRound(
  core: RsCoreApi,
  hostIds: readonly string[],
  now: () => Micros,
): Promise<LocalReach> {
  const byProbe = new Map<string, (event: ProbeResult) => void>();
  // Rust may emit results before diag.probe resolves with the probe id.
  const early = new Map<string, ProbeResult[]>();
  const unsubscribe = core.events.subscribe(event => {
    if (event.type !== 'probe.result') {
      return;
    }
    const handler = byProbe.get(event.probeId);
    if (handler !== undefined) {
      handler(event);
    } else if (event.hostId !== null) {
      early.get(event.hostId)?.push(event);
    }
  });

  const probeOne = (hostId: string) =>
    new Promise<boolean>(resolve => {
      let settled = false;
      let probeId: string | null = null;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const finish = (online: boolean) => {
        if (settled) {
          return;
        }
        settled = true;
        clearTimeout(timer);
        early.delete(hostId);
        if (probeId !== null) {
          byProbe.delete(probeId);
        }
        resolve(online);
      };
      const onResult = (event: ProbeResult) => {
        if (event.outcome === 'ok') {
          finish(true);
        } else if (event.last) {
          finish(false);
        }
      };
      timer = setTimeout(() => finish(false), REACH_TIMEOUT_MS);
      early.set(hostId, []);
      let pending: Promise<string>;
      try {
        pending = core.diag.probe(hostId);
      } catch {
        finish(false);
        return;
      }
      pending.then(
        id => {
          if (settled) {
            return;
          }
          probeId = id;
          byProbe.set(id, onResult);
          const buffered = early.get(hostId) ?? [];
          early.delete(hostId);
          for (const event of buffered) {
            if (event.probeId === id) {
              onResult(event);
            }
          }
        },
        () => finish(false),
      );
    });

  try {
    const reach: LocalReach = {};
    let next = 0;
    const worker = async () => {
      while (next < hostIds.length) {
        const hostId = hostIds[next];
        next += 1;
        if (hostId === undefined) {
          continue;
        }
        const online = await probeOne(hostId);
        reach[hostId] = { online, at: now() };
      }
    };
    await Promise.all(
      Array.from(
        { length: Math.min(REACH_CONCURRENCY, hostIds.length) },
        worker,
      ),
    );
    return reach;
  } finally {
    unsubscribe();
  }
}

const rounds = new WeakMap<RsCoreApi, Promise<unknown>>();

/**
 * One round of `diag.probe`: at most REACH_CONCURRENCY hosts in flight, each
 * capped at REACH_TIMEOUT_MS. A timeout, a failed call or a final result
 * without `ok` all count as offline. Rounds on one core run one at a time, so
 * a new host list or a manual refresh cannot raise the cap.
 */
export function probeReach(
  core: RsCoreApi,
  hostIds: readonly string[],
  now: () => Micros = nowUs,
): Promise<LocalReach> {
  const previous = rounds.get(core) ?? Promise.resolve();
  const round = previous
    .catch(() => undefined)
    .then(() => probeRound(core, hostIds, now));
  rounds.set(core, round);
  return round;
}

/**
 * Probes paired Hosts every REACH_INTERVAL_MS while `visible` and the app is
 * in the foreground. Results persist in `cache.localReach`, which also seeds
 * the first render.
 */
export function useLocalReach(
  hostIds: readonly string[],
  visible: boolean,
): { reach: LocalReach; refresh: () => Promise<unknown> } {
  const core = useCore();
  const kv = useKv();
  const signature = [...new Set(hostIds)].sort().join('\n');
  const ids = useMemo(
    () => (signature === '' ? [] : signature.split('\n')),
    [signature],
  );
  const cached = useMemo(() => {
    const stored = readLocalReach(kv);
    const reach: LocalReach = {};
    for (const hostId of ids) {
      const entry = stored[hostId];
      if (entry !== undefined) {
        reach[hostId] = entry;
      }
    }
    return reach;
  }, [kv, ids]);
  const query = useQuery({
    queryKey: ['localReach', signature],
    queryFn: async () => {
      const round = await probeReach(core, ids);
      kv.setJSON(LOCAL_REACH_KEY, { ...readLocalReach(kv), ...round });
      return round;
    },
    enabled: visible && ids.length > 0,
    staleTime: REACH_INTERVAL_MS,
    refetchInterval: REACH_INTERVAL_MS,
    refetchIntervalInBackground: false,
    retry: false,
  });
  return { reach: query.data ?? cached, refresh: query.refetch };
}
