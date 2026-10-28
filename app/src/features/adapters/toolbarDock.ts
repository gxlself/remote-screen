import { useCallback, useState } from 'react';
import { useKv } from '../storage/StorageContext';
import type { KvStore } from '../storage/kv';
import { TOOLBAR_DOCKS, type ToolbarDock } from './logic';

/** This phone's toolbar edge; local only, never synced. */
export const TOOLBAR_DOCK_KEY = 'adapters.toolbarDock';

export function readToolbarDock(kv: KvStore): ToolbarDock {
  const value = kv.getJSON<unknown>(TOOLBAR_DOCK_KEY);
  return TOOLBAR_DOCKS.find(dock => dock === value) ?? 'bottom';
}

export function useToolbarDock(): [ToolbarDock, (dock: ToolbarDock) => void] {
  const kv = useKv();
  const [dock, setDock] = useState(() => readToolbarDock(kv));
  const save = useCallback(
    (next: ToolbarDock) => {
      setDock(next);
      kv.setJSON(TOOLBAR_DOCK_KEY, next);
    },
    [kv],
  );
  return [dock, save];
}
