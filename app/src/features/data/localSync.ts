import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { KvStore } from '../storage/kv';
import type { SyncValue } from '../../native/rscore/types';

export type SyncNs =
  | 'snippets'
  | 'host_notes'
  | 'servers'
  | 'settings'
  | 'networks';
export const LOCAL_SYNC_PREFIX = 'sync.local.';
export const SYNC_NAMESPACES: readonly SyncNs[] = [
  'snippets',
  'host_notes',
  'servers',
  'settings',
  'networks',
];

function isSyncValue(value: unknown): value is SyncValue {
  return (
    typeof value === 'object' &&
    value !== null &&
    Number.isSafeInteger((value as SyncValue).version) &&
    (value as SyncValue).version >= 1
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function logWarn(core: RsCoreApi, msg: string): void {
  try {
    core.logs.write('warn', 'sync', msg);
  } catch {
    // Logging is best effort and must not fail the write it reports on.
  }
}

export function localSyncGet<T>(kv: KvStore, ns: SyncNs): SyncValue<T> | null {
  const stored = kv.getJSON<unknown>(LOCAL_SYNC_PREFIX + ns);
  return isSyncValue(stored) ? (stored as SyncValue<T>) : null;
}

/** A stale version throws a conflict with the current value. */
export function localSyncPut<T>(
  kv: KvStore,
  ns: SyncNs,
  value: T,
  version: number,
): SyncValue<T> {
  const current = localSyncGet<T>(kv, ns);
  if (version !== (current?.version ?? 0)) {
    throw new RsCoreError(
      'conflict',
      'sync version conflict',
      409,
      'conflict',
      current ?? undefined,
    );
  }
  const next: SyncValue<T> = { version: version + 1, value };
  kv.setJSON(LOCAL_SYNC_PREFIX + ns, next);
  return next;
}

/** Rust resolves manual SSH servers from its own copy, which cannot read MMKV. */
function pushServers(core: RsCoreApi, value: unknown): void {
  try {
    core.settings.set('sync.servers', value);
  } catch (error) {
    logWarn(core, `sync.servers push failed: ${errorMessage(error)}`);
  }
}

export function localSyncWrite<T>(
  core: RsCoreApi,
  kv: KvStore,
  ns: SyncNs,
  value: T,
  version: number,
): SyncValue<T> {
  const saved = localSyncPut(kv, ns, value, version);
  if (ns === 'servers') {
    pushServers(core, saved.value);
  }
  return saved;
}
