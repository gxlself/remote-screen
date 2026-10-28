import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { SyncValue } from '../../native/rscore/types';
import { getSettingsStorage } from '../settings/store';
import type { KvStore } from '../storage/kv';
import { localSyncGet, localSyncWrite } from './localSync';
import type { SyncNs } from './localSync';

/** This phone's storage, which holds every synced namespace. */
function boundSyncKv(): KvStore {
  const kv = getSettingsStorage();
  if (kv === undefined) {
    throw new RsCoreError('internal', 'sync storage is not bound');
  }
  return kv;
}

/** A stale version answers with a conflict carrying `current`; retry once from it. */
export async function putSyncWithRetry<T>(
  core: RsCoreApi,
  ns: SyncNs,
  update: (cur: T | null) => T,
  kv: KvStore = boundSyncKv(),
): Promise<SyncValue<T>> {
  const current = localSyncGet<T>(kv, ns);
  const value = update(current?.value ?? null);
  try {
    return localSyncWrite(core, kv, ns, value, current?.version ?? 0);
  } catch (error) {
    if (!(error instanceof RsCoreError) || error.kind !== 'conflict') {
      throw error;
    }
    const latest = error.current as SyncValue<T> | undefined;
    // Retry once using the value returned with the conflict, without another read.
    return localSyncWrite(
      core,
      kv,
      ns,
      update(latest?.value ?? null),
      latest?.version ?? 0,
    );
  }
}
