import { useCallback, useMemo } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import { useSync } from '../data/queries';
import { qk } from '../data/queryClient';
import { putSyncWithRetry } from '../data/syncRetry';
import type { ManualComputer, ManualServer, ServersValue } from './manual';
import {
  manualComputers,
  manualServers,
  parseServersValue,
  removeEntry,
  upsertEntry,
} from './manual';

export function useManualServers(): {
  value: ServersValue;
  servers: ManualServer[];
  computers: ManualComputer[];
  isLoading: boolean;
  saveServer(e: ManualServer): Promise<void>;
  removeServer(id: string): Promise<void>;
  saveComputer(e: ManualComputer): Promise<void>;
  removeComputer(id: string): Promise<void>;
} {
  const core = useCore();
  const queryClient = useQueryClient();
  const sync = useSync<unknown>('servers');
  const value = useMemo(() => parseServersValue(sync.data?.value), [sync.data]);
  const servers = useMemo(() => manualServers(value), [value]);
  const computers = useMemo(() => manualComputers(value), [value]);

  const write = useCallback(
    async (update: (current: ServersValue) => ServersValue): Promise<void> => {
      const result = await putSyncWithRetry<ServersValue>(
        core,
        'servers',
        current => update(parseServersValue(current)),
      );
      queryClient.setQueryData(qk.sync('servers'), result);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: qk.sshServers }),
        queryClient.invalidateQueries({ queryKey: qk.directory }),
      ]);
    },
    [core, queryClient],
  );

  const saveServer = useCallback(
    (entry: ManualServer) => write(current => upsertEntry(current, entry)),
    [write],
  );
  const removeServer = useCallback(
    (id: string) => write(current => removeEntry(current, id)),
    [write],
  );
  const saveComputer = useCallback(
    (entry: ManualComputer) => write(current => upsertEntry(current, entry)),
    [write],
  );
  const removeComputer = useCallback(
    (id: string) => write(current => removeEntry(current, id)),
    [write],
  );

  return {
    value,
    servers,
    computers,
    isLoading: sync.isLoading,
    saveServer,
    removeServer,
    saveComputer,
    removeComputer,
  };
}
