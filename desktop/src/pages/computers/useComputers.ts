import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import type { PairedHost } from '@app/native/rscore/types';

import { useCore } from '../../core/CoreContext';
import { useCoreEvents, useSessionEvents } from '../hooks';

import { computerRows, type ComputerRow } from './model';

export const REFRESH_MS = 30_000;

/**
 * Paired hosts; null until the first answer. A failed request keeps the previous list.
 * Refreshes on mount, every 30 s, when the window gets focus, after a successful pairing and
 * after a session ends.
 */
export function useComputers(): ComputerRow[] | null {
  const core = useCore();
  const [paired, setPaired] = useState<PairedHost[] | null>(null);
  const latest = useRef(0);

  const refresh = useCallback(async () => {
    latest.current += 1;
    const request = latest.current;
    const hosts = await core.hosts.paired().catch(() => null);
    if (request === latest.current) {
      setPaired((prev) => hosts ?? prev ?? []);
    }
  }, [core]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    const onFocus = () => void refresh();
    window.addEventListener('focus', onFocus);
    return () => {
      clearInterval(timer);
      window.removeEventListener('focus', onFocus);
    };
  }, [refresh]);

  useCoreEvents((event) => {
    if (event.type === 'pairing.done' && event.outcome.status === 'ok') {
      void refresh();
    }
  });
  useSessionEvents((event) => {
    if (event.type === 'ended') {
      void refresh();
    }
  });

  return useMemo(() => (paired === null ? null : computerRows(paired)), [paired]);
}
