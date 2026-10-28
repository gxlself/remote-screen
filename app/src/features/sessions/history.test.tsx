import { act, renderHook } from '@testing-library/react-native';
import type { ReactNode } from 'react';
import { MemoryKv } from '../storage/kv';
import { StorageProvider } from '../storage/StorageContext';
import {
  ADAPTER_LIVE_KEY,
  clearAdapterLive,
  clearHistory,
  markAdapterLive,
  useHistory,
  HISTORY_KEY,
  HISTORY_MAX,
  LIVE_SNAPSHOT_KEY,
  appendHistory,
  recoverUnclean,
  readHistory,
  saveLiveSnapshot,
} from './history';
import type { HistoryEntry, LiveSnapshotEntry } from './history';

function entry(id: string, endedAtUs = 100): HistoryEntry {
  return {
    id,
    kind: 'remote',
    targetId: 'host-1',
    name: 'Studio Mac mini',
    startedAtUs: 1,
    endedAtUs,
    durationS: 1,
  };
}

function snapshot(id: string): LiveSnapshotEntry {
  return {
    id,
    kind: 'remote',
    targetId: 'host-1',
    name: 'Studio Mac mini',
    startedAtUs: 1,
  };
}

it('returns history newest first without writing', () => {
  const kv = new MemoryKv();
  appendHistory(kv, entry('old', 100));
  appendHistory(kv, entry('new', 200));
  expect(readHistory(kv).map(item => item.id)).toEqual(['new', 'old']);
});

it('does not duplicate an entry with the same id', () => {
  const kv = new MemoryKv();
  appendHistory(kv, entry('same', 100));
  appendHistory(kv, { ...entry('same', 200), name: 'changed' });
  expect(readHistory(kv)).toEqual([entry('same')]);
});

it('keeps only the newest HISTORY_MAX entries', () => {
  const kv = new MemoryKv();
  for (let index = 0; index < 31; index += 1) {
    appendHistory(kv, entry(`e${index}`, index));
  }
  const history = readHistory(kv);
  expect(history).toHaveLength(HISTORY_MAX);
  expect(history[0].id).toBe('e30');
  expect(history.at(-1)?.id).toBe('e1');
});

it('returns an empty list for malformed KV data', () => {
  const kv = new MemoryKv({ [HISTORY_KEY]: '{' });
  expect(readHistory(kv)).toEqual([]);
});

it('recovers a snapshot as unclean history and clears it once', () => {
  const kv = new MemoryKv();
  saveLiveSnapshot(kv, [snapshot('a'), snapshot('b')]);
  expect(kv.getJSON(LIVE_SNAPSHOT_KEY)).toHaveLength(2);

  const recovered = recoverUnclean(kv, 2_000_000);
  expect(recovered).toHaveLength(2);
  expect(recovered.every(item => item.unclean)).toBe(true);
  expect(recovered.every(item => item.endedAtUs === 2_000_000)).toBe(true);
  expect(kv.getString(LIVE_SNAPSHOT_KEY)).toBeUndefined();

  const again = recoverUnclean(kv, 3_000_000);
  expect(again).toHaveLength(2);
});

it('clears an empty snapshot', () => {
  const kv = new MemoryKv();
  saveLiveSnapshot(kv, [snapshot('a')]);
  saveLiveSnapshot(kv, []);
  expect(kv.getString(LIVE_SNAPSHOT_KEY)).toBeUndefined();
});

it('filters invalid entries while keeping valid history', () => {
  const kv = new MemoryKv();
  kv.setJSON(HISTORY_KEY, [entry('valid'), null, { id: 'broken' }]);
  expect(readHistory(kv).map(item => item.id)).toEqual(['valid']);
});

function adapterSnapshot(id: string): LiveSnapshotEntry {
  return {
    id,
    kind: 'adapter',
    targetId: 'manual:nas',
    name: 'NAS Mac',
    startedAtUs: 1,
    protocol: 'vnc',
  };
}

it('keeps adapter live markers apart and clears them one by one', () => {
  const kv = new MemoryKv();
  markAdapterLive(kv, adapterSnapshot('adapter:1'));
  markAdapterLive(kv, adapterSnapshot('adapter:2'));
  markAdapterLive(kv, adapterSnapshot('adapter:1'));
  expect(kv.getJSON(ADAPTER_LIVE_KEY)).toEqual([
    adapterSnapshot('adapter:2'),
    adapterSnapshot('adapter:1'),
  ]);

  clearAdapterLive(kv, 'adapter:2');
  expect(kv.getJSON(ADAPTER_LIVE_KEY)).toEqual([adapterSnapshot('adapter:1')]);
  clearAdapterLive(kv, 'adapter:1');
  expect(kv.getJSON(ADAPTER_LIVE_KEY)).toBeUndefined();
});

it('recovers adapter sessions the app did not get to close (H06)', () => {
  const kv = new MemoryKv();
  markAdapterLive(kv, adapterSnapshot('adapter:1'));
  saveLiveSnapshot(kv, [snapshot('remote:1')]);

  const recovered = recoverUnclean(kv, 5_000_001);
  expect(recovered.map(item => item.id).sort()).toEqual([
    'adapter:1',
    'remote:1',
  ]);
  expect(recovered.find(item => item.id === 'adapter:1')).toMatchObject({
    kind: 'adapter',
    protocol: 'vnc',
    durationS: 5,
    unclean: true,
  });
  expect(kv.getJSON(ADAPTER_LIVE_KEY)).toBeUndefined();
});

it('drops entries with an unknown protocol', () => {
  const kv = new MemoryKv();
  kv.setJSON(HISTORY_KEY, [
    { ...entry('ok'), kind: 'adapter', protocol: 'rdp' },
    { ...entry('bad'), kind: 'adapter', protocol: 'ssh' },
  ]);
  expect(readHistory(kv).map(item => item.id)).toEqual(['ok']);
});

it('lists every kind by default and only the asked kinds on request', async () => {
  const kv = new MemoryKv();
  appendHistory(kv, entry('remote', 100));
  appendHistory(kv, { ...entry('adapter', 200), kind: 'adapter' });
  function Wrapper({ children }: { children: ReactNode }) {
    return <StorageProvider kv={kv}>{children}</StorageProvider>;
  }

  // H06: the lists open adapter entries now, so they show them too.
  const listed = await renderHook(() => useHistory(['remote', 'ssh']), {
    wrapper: Wrapper,
  });
  expect(listed.result.current.map(item => item.id)).toEqual(['remote']);
  const all = await renderHook(() => useHistory(), {
    wrapper: Wrapper,
  });
  expect(all.result.current.map(item => item.id)).toEqual([
    'adapter',
    'remote',
  ]);

  await act(async () => {
    appendHistory(kv, { ...entry('later', 300), kind: 'adapter' });
  });
  expect(listed.result.current.map(item => item.id)).toEqual(['remote']);
  expect(all.result.current.map(item => item.id)).toEqual([
    'later',
    'adapter',
    'remote',
  ]);
});

it('clears the list and tells every reader', async () => {
  const kv = new MemoryKv();
  appendHistory(kv, entry('remote', 100));
  function Wrapper({ children }: { children: ReactNode }) {
    return <StorageProvider kv={kv}>{children}</StorageProvider>;
  }
  const listed = await renderHook(() => useHistory(), { wrapper: Wrapper });
  expect(listed.result.current).toHaveLength(1);

  await act(async () => {
    clearHistory(kv);
  });
  expect(listed.result.current).toEqual([]);
  expect(readHistory(kv)).toEqual([]);
});
