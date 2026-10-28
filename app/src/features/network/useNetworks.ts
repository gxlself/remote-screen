import { useCallback, useMemo } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import type { NetworkEntry } from '../settings/schema';
import { useSetting } from '../settings/store';
import { qk } from '../data/queryClient';
import { useSync } from '../data/queries';
import { putSyncWithRetry } from '../data/syncRetry';
import { parseNetworks, removeNetwork, upsertNetwork } from './networks';
import type { NetworksValue } from './networks';

export function useNetworks(): {
  items: NetworkEntry[];
  isLoading: boolean;
  save(entry: NetworkEntry): Promise<void>;
  remove(id: string): Promise<void>;
} {
  const core = useCore();
  const queryClient = useQueryClient();
  const query = useSync<unknown>('networks');
  const [, setLocalNetworks] = useSetting('networks');
  const parsed = useMemo(() => parseNetworks(query.data?.value), [query.data]);

  const write = useCallback(
    async (apply: (current: NetworksValue) => NetworksValue) => {
      const saved = await putSyncWithRetry<unknown>(core, 'networks', current =>
        apply(parseNetworks(current)),
      );
      const next = parseNetworks(saved.value);
      queryClient.setQueryData(qk.sync('networks'), saved);
      setLocalNetworks(next.items);
    },
    [core, queryClient, setLocalNetworks],
  );

  const save = useCallback(
    (entry: NetworkEntry) => write(current => upsertNetwork(current, entry)),
    [write],
  );

  const remove = useCallback(
    (id: string) => write(current => removeNetwork(current, id)),
    [write],
  );

  return {
    items: parsed.items,
    isLoading: query.isLoading,
    save,
    remove,
  };
}
