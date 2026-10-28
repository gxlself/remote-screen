import { useCallback, useEffect, useMemo } from 'react';
import { AppState } from 'react-native';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type {
  AdapterErrorInfo,
  CoreEvent,
  SessionId,
} from '../../native/rscore/types';
import { useCoreEvent } from '../events/EventsProvider';
import { finishAdapterHistory } from '../sessions/useAdapterHistory';
import type { CollapsedKeep } from '../settings/schema';
import { useSetting } from '../settings/store';
import { useKv } from '../storage/StorageContext';
import { adapterLive, selectCollapsed, useAdapterLive } from './liveStore';
import { LOST_IN_BACKGROUND } from './logic';

/**
 * How long a folded-away session keeps its connection by default (设置 ›
 * 收起的会话保持). After that it is closed to save the battery and the
 * computer's resources; returning to it connects again by itself.
 */
export const COLLAPSED_KEEP_MS = 2 * 60 * 60 * 1000;

/** Each choice of the setting; null keeps the session until it is closed. */
export const COLLAPSED_KEEP: Record<CollapsedKeep, number | null> = {
  '10m': 10 * 60 * 1000,
  '30m': 30 * 60 * 1000,
  '2h': COLLAPSED_KEEP_MS,
  always: null,
};

export const RECLAIMED: AdapterErrorInfo = {
  code: 'io',
  rsCode: ADAPTER_RS_E.broken,
  message: 'closed after staying collapsed',
};

function closeQuietly(core: RsCoreApi, id: SessionId): void {
  // A folded-away session is paused; take it out of the pause before closing.
  try {
    core.sessions.background(id, false);
  } catch {
    // The native session may already be gone.
  }
  try {
    core.adapters?.close(id);
  } catch {
    // The native session may already be gone.
  }
}

function aliveIds(core: RsCoreApi): Set<SessionId> | null {
  try {
    const list = core.adapters?.list();
    return list === undefined
      ? null
      : new Set(
          list
            .filter(summary => summary.phase !== 'ended')
            .map(summary => summary.sessionId),
        );
  } catch {
    return null;
  }
}

/**
 * Looks after folded-away adapter sessions while no page shows them: follows
 * their state, closes them after the kept time (session.collapsedKeep),
 * notices the ones the system killed in the background, and writes 最近 for
 * the ones that are gone for good. Mounted once, next to the history recorder.
 */
export function useAdapterLiveKeeper(): void {
  const core = useCore();
  const kv = useKv();
  const records = useAdapterLive(selectCollapsed);
  const [keep] = useSetting('session.collapsedKeep');
  const keepMs = COLLAPSED_KEEP[keep];

  const onEvent = (event: CoreEvent) => {
    if (event.type !== 'adapter.state' && event.type !== 'adapter.screens') {
      return;
    }
    const before = adapterLive.peek(event.sessionId);
    if (before === undefined || before.phase === 'ended') {
      return;
    }
    adapterLive.apply(event);
    const after = adapterLive.peek(event.sessionId);
    // A clean end leaves nothing to come back to.
    if (
      after?.phase === 'ended' &&
      (after.error === null || after.error.code === 'cancelled')
    ) {
      adapterLive.take(event.sessionId);
      finishAdapterHistory(kv, after.history);
    }
  };
  useCoreEvent('adapter.state', onEvent);
  useCoreEvent('adapter.screens', onEvent);

  const reclaim = useCallback(() => {
    if (keepMs === null) {
      return;
    }
    const nowMs = Date.now();
    for (const record of selectCollapsed(adapterLive.getState())) {
      if (
        record.phase !== 'ended' &&
        nowMs >= record.collapsedAtUs / 1000 + keepMs
      ) {
        adapterLive.end(record.sessionId, RECLAIMED, undefined, false);
        closeQuietly(core, record.sessionId);
      }
    }
  }, [core, keepMs]);

  const nextDeadlineMs = useMemo(() => {
    if (keepMs === null) {
      return null;
    }
    const deadlines = records
      .filter(record => record.phase !== 'ended')
      .map(record => record.collapsedAtUs / 1000 + keepMs);
    return deadlines.length === 0 ? null : Math.min(...deadlines);
  }, [keepMs, records]);

  useEffect(() => {
    if (nextDeadlineMs === null) {
      return;
    }
    const timer = setTimeout(
      reclaim,
      Math.max(0, nextDeadlineMs - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [nextDeadlineMs, reclaim]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => {
      if (state !== 'active') {
        return;
      }
      reclaim();
      const live = selectCollapsed(adapterLive.getState()).filter(
        record => record.phase !== 'ended',
      );
      if (live.length === 0) {
        return;
      }
      // Sockets often die while the app sleeps without an event reaching us.
      const alive = aliveIds(core);
      if (alive === null) {
        return;
      }
      for (const record of live) {
        if (!alive.has(record.sessionId)) {
          adapterLive.end(record.sessionId, LOST_IN_BACKGROUND);
        }
      }
    });
    return () => subscription.remove();
  }, [core, reclaim]);
}

/** Disconnects a folded-away session from Home or the session tab. */
export function useDisconnectCollapsed(): (id: SessionId) => void {
  const core = useCore();
  const kv = useKv();
  return useCallback(
    (id: SessionId) => {
      const record = adapterLive.take(id);
      if (record === undefined) {
        return;
      }
      if (record.phase !== 'ended') {
        closeQuietly(core, id);
      }
      finishAdapterHistory(kv, { ...record.history });
    },
    [core, kv],
  );
}
