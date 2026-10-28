import { useEffect, useState } from 'react';
import type { AdapterProtocol, NetPath } from '../../native/rscore/types';
import type { KvStore } from '../storage/kv';
import { useKv } from '../storage/StorageContext';

/** adapter：不经过 Host 的屏幕共享 / 远程桌面（targetId 是电脑列表的 key）。 */
export type HistoryKind = 'remote' | 'ssh' | 'adapter';

export type HistoryEntry = {
  id: string;
  kind: HistoryKind;
  targetId: string;
  name: string;
  startedAtUs: number;
  endedAtUs: number;
  durationS: number;
  path?: NetPath;
  derpRegion?: string;
  endReason?: string;
  code?: number;
  unclean?: boolean;
  protocol?: AdapterProtocol;
};

export type LiveSnapshotEntry = Omit<HistoryEntry, 'endedAtUs' | 'durationS'>;

export const HISTORY_MAX = 30;
export const HISTORY_KEY = 'sessions.history';
export const LIVE_SNAPSHOT_KEY = 'sessions.liveSnapshot';
/** Adapter sessions live outside the session stores, so they keep their own. */
export const ADAPTER_LIVE_KEY = 'sessions.adapterLive';
/** What the lists show: every kind, adapters open through `connect`. */
export const LISTED_KINDS: readonly HistoryKind[] = [
  'remote',
  'ssh',
  'adapter',
];

const listeners = new Set<(kv: KvStore) => void>();

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function isNetPath(value: unknown): value is NetPath {
  return (
    value === 'tailnetDirect' ||
    value === 'tailnetDerp' ||
    value === 'intranet' ||
    value === 'lan' ||
    value === 'manual'
  );
}

function hasSnapshotFields(value: unknown): value is LiveSnapshotEntry {
  if (!isObject(value)) {
    return false;
  }
  if (
    typeof value.id !== 'string' ||
    (value.kind !== 'remote' &&
      value.kind !== 'ssh' &&
      value.kind !== 'adapter') ||
    typeof value.targetId !== 'string' ||
    typeof value.name !== 'string' ||
    !isNumber(value.startedAtUs)
  ) {
    return false;
  }
  if (value.path !== undefined && !isNetPath(value.path)) {
    return false;
  }
  return (
    (value.derpRegion === undefined || typeof value.derpRegion === 'string') &&
    (value.endReason === undefined || typeof value.endReason === 'string') &&
    (value.code === undefined || isNumber(value.code)) &&
    (value.unclean === undefined || typeof value.unclean === 'boolean') &&
    (value.protocol === undefined ||
      value.protocol === 'vnc' ||
      value.protocol === 'rdp')
  );
}

function isHistoryEntry(value: unknown): value is HistoryEntry {
  return (
    hasSnapshotFields(value) &&
    isNumber((value as Record<string, unknown>).endedAtUs) &&
    isNumber((value as Record<string, unknown>).durationS)
  );
}

function readHistoryUnchecked(kv: KvStore): HistoryEntry[] {
  const raw = kv.getJSON<unknown>(HISTORY_KEY);
  if (!Array.isArray(raw)) {
    return [];
  }
  return raw.filter(isHistoryEntry).sort((left, right) => {
    if (left.endedAtUs !== right.endedAtUs) {
      return right.endedAtUs - left.endedAtUs;
    }
    return right.startedAtUs - left.startedAtUs;
  });
}

export function readHistory(kv: KvStore): HistoryEntry[] {
  try {
    const raw = kv.getString(HISTORY_KEY);
    if (raw === undefined) {
      return [];
    }
    JSON.parse(raw);
  } catch {
    return [];
  }
  return readHistoryUnchecked(kv);
}

function notify(kv: KvStore): void {
  for (const listener of [...listeners]) {
    listener(kv);
  }
}

export function appendHistory(
  kv: KvStore,
  entry: HistoryEntry,
): HistoryEntry[] {
  const current = readHistory(kv);
  if (current.some(item => item.id === entry.id)) {
    return current;
  }
  const next = [entry, ...current]
    .sort((left, right) => {
      if (left.endedAtUs !== right.endedAtUs) {
        return right.endedAtUs - left.endedAtUs;
      }
      return right.startedAtUs - left.startedAtUs;
    })
    .slice(0, HISTORY_MAX);
  kv.setJSON(HISTORY_KEY, next);
  notify(kv);
  return next;
}

/** Forgets this phone's 最近 list. */
export function clearHistory(kv: KvStore): void {
  kv.delete(HISTORY_KEY);
  notify(kv);
}

export function saveLiveSnapshot(kv: KvStore, live: LiveSnapshotEntry[]): void {
  if (live.length === 0) {
    kv.delete(LIVE_SNAPSHOT_KEY);
    return;
  }
  kv.setJSON(LIVE_SNAPSHOT_KEY, live);
}

function readSnapshot(kv: KvStore, key: string): LiveSnapshotEntry[] {
  const raw = kv.getJSON<unknown>(key);
  if (!Array.isArray(raw)) {
    return [];
  }
  return raw.filter(hasSnapshotFields);
}

/** Marks an adapter session as running, for recovery if the app dies. */
export function markAdapterLive(kv: KvStore, entry: LiveSnapshotEntry): void {
  const others = readSnapshot(kv, ADAPTER_LIVE_KEY).filter(
    item => item.id !== entry.id,
  );
  kv.setJSON(ADAPTER_LIVE_KEY, [...others, entry]);
}

export function clearAdapterLive(kv: KvStore, id: string): void {
  const rest = readSnapshot(kv, ADAPTER_LIVE_KEY).filter(
    item => item.id !== id,
  );
  if (rest.length === 0) {
    kv.delete(ADAPTER_LIVE_KEY);
  } else {
    kv.setJSON(ADAPTER_LIVE_KEY, rest);
  }
}

export function recoverUnclean(kv: KvStore, nowUs: number): HistoryEntry[] {
  const recovered = [
    ...readSnapshot(kv, LIVE_SNAPSHOT_KEY),
    ...readSnapshot(kv, ADAPTER_LIVE_KEY),
  ].map(entry => ({
    ...entry,
    endedAtUs: nowUs,
    durationS: Math.max(0, Math.floor((nowUs - entry.startedAtUs) / 1e6)),
    unclean: true,
  }));

  if (recovered.length > 0) {
    const current = readHistory(kv);
    const existing = new Set(current.map(entry => entry.id));
    const next = [
      ...recovered.filter(entry => !existing.has(entry.id)),
      ...current,
    ]
      .sort((left, right) => {
        if (left.endedAtUs !== right.endedAtUs) {
          return right.endedAtUs - left.endedAtUs;
        }
        return right.startedAtUs - left.startedAtUs;
      })
      .slice(0, HISTORY_MAX);
    kv.setJSON(HISTORY_KEY, next);
  }

  kv.delete(LIVE_SNAPSHOT_KEY);
  kv.delete(ADAPTER_LIVE_KEY);
  notify(kv);
  return readHistory(kv);
}

/** History of the given kinds, newest first; LISTED_KINDS by default. */
export function useHistory(
  kinds: readonly HistoryKind[] = LISTED_KINDS,
): HistoryEntry[] {
  const kv = useKv();
  const kindsKey = kinds.join(',');
  const [entries, setEntries] = useState<HistoryEntry[]>(() =>
    readHistory(kv).filter(entry => kinds.includes(entry.kind)),
  );

  useEffect(() => {
    const wanted = kindsKey.split(',');
    const read = () =>
      readHistory(kv).filter(entry => wanted.includes(entry.kind));
    const listener = (source: KvStore) => {
      if (source === kv) {
        setEntries(read());
      }
    };
    setEntries(read());
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, [kindsKey, kv]);

  return entries;
}
