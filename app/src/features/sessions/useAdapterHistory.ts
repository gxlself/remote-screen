import { useEffect, useState } from 'react';
import type {
  AdapterErrorInfo,
  AdapterTarget,
  SessionId,
} from '../../native/rscore/types';
import { entryName, entryProtocol } from '../native/entries';
import type { ComputerEntry } from '../native/types';
import { useComputerEntries } from '../native/useComputerEntries';
import { useKv } from '../storage/StorageContext';
import type { KvStore } from '../storage/kv';
import { appendHistory, clearAdapterLive, markAdapterLive } from './history';
import type { HistoryEntry, LiveSnapshotEntry } from './history';

function entryMatches(entry: ComputerEntry, target: AdapterTarget): boolean {
  if (entryProtocol(entry) !== target.protocol) {
    return false;
  }
  switch (entry.kind) {
    case 'host':
      return (
        (target.hostId !== undefined &&
          entry.computer.hostId === target.hostId) ||
        entry.computer.addresses.some(address => address.addr === target.addr)
      );
    case 'tailnet':
      return entry.device.addrs.includes(target.addr);
    case 'manual':
      return entry.manual.host === target.addr;
  }
}

/** The computer list entry this adapter target was opened from, if any. */
export function adapterEntryFor(
  entries: readonly ComputerEntry[],
  target: AdapterTarget,
): ComputerEntry | undefined {
  return entries.find(entry => entryMatches(entry, target));
}

/** History identity: the list key, so a later list can reopen the same row. */
export function adapterTargetId(
  entry: ComputerEntry | undefined,
  target: AdapterTarget,
): string {
  if (entry !== undefined) {
    return entry.key;
  }
  return target.hostId !== undefined
    ? `host:${target.hostId}`
    : `addr:${target.protocol}:${target.addr}`;
}

export type AdapterHistoryTracker = {
  live: LiveSnapshotEntry | null;
  stoppedAtUs: number | null;
  error: AdapterErrorInfo | null;
  name: string;
  targetId: string;
};

function nowUs(): number {
  return Date.now() * 1000;
}

/** Writes the 最近 entry a tracker holds, once; nothing before a picture. */
export function finishAdapterHistory(
  kv: KvStore,
  tracker: AdapterHistoryTracker,
): void {
  const live = tracker.live;
  if (live === null) {
    return;
  }
  tracker.live = null;
  const endedAtUs = tracker.stoppedAtUs ?? nowUs();
  const done: HistoryEntry = {
    ...live,
    name: tracker.name,
    targetId: tracker.targetId,
    endedAtUs,
    durationS: Math.max(0, Math.floor((endedAtUs - live.startedAtUs) / 1e6)),
  };
  if (tracker.error !== null) {
    done.endReason = tracker.error.code;
    if (tracker.error.rsCode > 0) {
      done.code = tracker.error.rsCode;
    }
  }
  appendHistory(kv, done);
  clearAdapterLive(kv, live.id);
}

/**
 * Writes one 最近 entry per adapter session (H06): it starts with the first
 * picture, survives reconnects, retries and folding the page away (V33), and
 * ends when the picture last stopped. A live marker lets the next launch
 * recover a killed app. The page hands the tracker on when it folds away:
 * clearing `live` keeps the unmount from writing the entry.
 */
export function useAdapterHistory(p: {
  target: AdapterTarget;
  /** Shown when the computer is not in the list (a typed address). */
  fallbackName: string;
  sessionId: SessionId | null;
  streaming: boolean;
  error: AdapterErrorInfo | null;
  /** The tracker of a session folded away earlier, picked up again. */
  resume?: AdapterHistoryTracker;
}): AdapterHistoryTracker {
  const kv = useKv();
  const entries = useComputerEntries();
  const entry = adapterEntryFor(entries, p.target);
  const name = entry === undefined ? p.fallbackName : entryName(entry);
  const targetId = adapterTargetId(entry, p.target);
  const [tracker] = useState<AdapterHistoryTracker>(() =>
    p.resume === undefined
      ? { live: null, stoppedAtUs: null, error: null, name, targetId }
      : { ...p.resume },
  );
  tracker.name = name;
  tracker.targetId = targetId;

  const { sessionId, streaming, error } = p;
  const protocol = p.target.protocol;
  useEffect(() => {
    if (streaming && sessionId !== null) {
      tracker.stoppedAtUs = null;
      tracker.error = null;
      if (tracker.live === null) {
        tracker.live = {
          id: `adapter:${sessionId}`,
          kind: 'adapter',
          targetId: tracker.targetId,
          name: tracker.name,
          startedAtUs: nowUs(),
          protocol,
        };
        markAdapterLive(kv, tracker.live);
      }
      return;
    }
    if (tracker.live !== null && tracker.stoppedAtUs === null) {
      tracker.stoppedAtUs = nowUs();
    }
  }, [kv, protocol, sessionId, streaming, tracker]);

  useEffect(() => {
    if (error !== null && error.code !== 'cancelled') {
      tracker.error = error;
    }
  }, [error, tracker]);

  useEffect(() => () => finishAdapterHistory(kv, tracker), [kv, tracker]);

  return tracker;
}
