import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import { useKv } from '../storage/StorageContext';
import { localSyncGet } from './localSync';
import type { SyncNs } from './localSync';
import { qk } from './queryClient';
import { putSyncWithRetry } from './syncRetry';

export function useSync<T>(ns: SyncNs) {
  const kv = useKv();
  return useQuery({
    queryKey: qk.sync(ns),
    queryFn: async () => localSyncGet<T>(kv, ns),
  });
}

export function usePutSync<T>(ns: SyncNs) {
  const core = useCore();
  const kv = useKv();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (update: (cur: T | null) => T) =>
      putSyncWithRetry(core, ns, update, kv),
    onSuccess: data => {
      queryClient.setQueryData(qk.sync(ns), data);
    },
  });
}
