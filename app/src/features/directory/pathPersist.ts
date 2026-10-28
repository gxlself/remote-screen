import type { Micros, NetPath, Transport } from '../../native/rscore/types';
import type { KvStore } from '../storage/kv';
import type { HostPath, PathStore } from './pathStore';

export const PATHS_KEY = 'path.byHost.v1';
// The design docs leave these open: forget a path after a day, coalesce
// writes for two seconds, and keep the 200 most recently updated hosts.
export const PATH_MAX_AGE_US = 24 * 60 * 60 * 1_000_000;
export const PATH_SAVE_DELAY_MS = 2_000;
export const PATH_MAX_HOSTS = 200;

const NET_PATHS: Record<NetPath, true> = {
  tailnetDirect: true,
  tailnetDerp: true,
  intranet: true,
  lan: true,
  manual: true,
};
const TRANSPORTS: Record<Transport, true> = { quic: true, tcp: true };

function oneOf<K extends string>(
  keys: Record<K, true>,
  value: unknown,
): value is K {
  return (
    typeof value === 'string' &&
    Object.prototype.hasOwnProperty.call(keys, value)
  );
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function parseHostPath(value: unknown): HostPath | null {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  const { path, latencyMs, transport, derpRegion, atUs } = value as Record<
    string,
    unknown
  >;
  if (
    !oneOf(NET_PATHS, path) ||
    !isFiniteNumber(latencyMs) ||
    !oneOf(TRANSPORTS, transport) ||
    !isFiniteNumber(atUs) ||
    (derpRegion !== undefined && typeof derpRegion !== 'string')
  ) {
    return null;
  }
  const hostPath: HostPath = { path, latencyMs, transport, atUs };
  if (derpRegion !== undefined) {
    hostPath.derpRegion = derpRegion;
  }
  return hostPath;
}

export function loadPaths(
  kv: KvStore,
  nowUs: Micros,
): Record<string, HostPath> {
  const stored = kv.getJSON<unknown>(PATHS_KEY);
  const byHost: Record<string, HostPath> = {};
  if (stored === null || typeof stored !== 'object' || Array.isArray(stored)) {
    return byHost;
  }
  for (const [hostId, value] of Object.entries(stored)) {
    const path = parseHostPath(value);
    if (path !== null && nowUs - path.atUs <= PATH_MAX_AGE_US) {
      byHost[hostId] = path;
    }
  }
  return byHost;
}

function newest(
  byHost: Record<string, HostPath>,
  limit: number,
): Record<string, HostPath> {
  const entries = Object.entries(byHost);
  if (entries.length <= limit) {
    return byHost;
  }
  entries.sort(([, a], [, b]) => b.atUs - a.atUs);
  return Object.fromEntries(entries.slice(0, limit));
}

/**
 * Saves byHost (never routes) after it changes. Changes within the delay share
 * one write; the timer is not restarted, since session.stats arrives at 2 Hz
 * and would otherwise postpone the write for a whole session. Detaching
 * flushes a pending write.
 */
export function attachPathPersistence(
  store: PathStore,
  kv: KvStore,
  opts?: { delayMs?: number; maxHosts?: number },
): () => void {
  const delayMs = opts?.delayMs ?? PATH_SAVE_DELAY_MS;
  const maxHosts = opts?.maxHosts ?? PATH_MAX_HOSTS;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const save = () => {
    timer = null;
    kv.setJSON(PATHS_KEY, newest(store.getState().byHost, maxHosts));
  };
  const unsubscribe = store.subscribe((state, previous) => {
    if (state.byHost !== previous.byHost && timer === null) {
      timer = setTimeout(save, delayMs);
    }
  });
  return () => {
    unsubscribe();
    if (timer !== null) {
      clearTimeout(timer);
      save();
    }
  };
}
