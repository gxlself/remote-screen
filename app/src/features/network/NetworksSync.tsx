import { useEffect, useMemo } from 'react';
import { useSetting } from '../settings/store';
import { useSync } from '../data/queries';
import { parseNetworks, sameNetworks } from './networks';

/** Mirrors the networks saved on this phone into the settings read offline. */
export function NetworksSync(): null {
  const query = useSync<unknown>('networks');
  const [localNetworks, setLocalNetworks] = useSetting('networks');
  const parsed = useMemo(() => parseNetworks(query.data?.value), [query.data]);
  const syncedItems =
    query.data?.value === undefined || query.data.value === null
      ? null
      : parsed.items;

  useEffect(() => {
    if (syncedItems === null || sameNetworks(syncedItems, localNetworks)) {
      return;
    }
    setLocalNetworks(syncedItems);
  }, [localNetworks, setLocalNetworks, syncedItems]);

  return null;
}

export default NetworksSync;
