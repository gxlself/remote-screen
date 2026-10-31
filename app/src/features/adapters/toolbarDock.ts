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

/** Collapsing leaves no handle; this device only, like the dock. */
export const TOOLBAR_HANDLE_HIDDEN_KEY = 'adapters.toolbarHandleHidden';
/** Set once the toast saying how to bring a hidden toolbar back has shown. */
export const TOOLBAR_HIDDEN_HINT_KEY = 'adapters.toolbarHiddenHint';

export function useToolbarHandleHidden(): [boolean, (hidden: boolean) => void] {
  const kv = useKv();
  const [hidden, setHidden] = useState(
    () => kv.getJSON<unknown>(TOOLBAR_HANDLE_HIDDEN_KEY) === true,
  );
  const save = useCallback(
    (next: boolean) => {
      setHidden(next);
      kv.setJSON(TOOLBAR_HANDLE_HIDDEN_KEY, next);
    },
    [kv],
  );
  return [hidden, save];
}
