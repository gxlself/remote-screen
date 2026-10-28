import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { MemoryKv } from '../storage/kv';
import { LOCAL_SYNC_PREFIX, localSyncGet, localSyncPut } from './localSync';

function thrown(fn: () => unknown): unknown {
  try {
    fn();
  } catch (error) {
    return error;
  }
  throw new Error('expected a throw');
}

describe('localSyncGet / localSyncPut', () => {
  it('reads an empty namespace as null', () => {
    expect(localSyncGet(new MemoryKv(), 'snippets')).toBeNull();
  });

  it('creates a namespace from version zero and stores it under its key', () => {
    const kv = new MemoryKv();

    expect(localSyncPut(kv, 'servers', { items: ['a'] }, 0)).toEqual({
      version: 1,
      value: { items: ['a'] },
    });
    expect(kv.keys()).toEqual([`${LOCAL_SYNC_PREFIX}servers`]);
    expect(kv.getJSON('sync.local.servers')).toEqual({
      version: 1,
      value: { items: ['a'] },
    });
    expect(localSyncPut(kv, 'servers', { items: ['b'] }, 1)).toEqual({
      version: 2,
      value: { items: ['b'] },
    });
    expect(localSyncGet(kv, 'servers')).toEqual({
      version: 2,
      value: { items: ['b'] },
    });
  });

  it('rejects a stale version with a structured conflict carrying current', () => {
    const kv = new MemoryKv();
    localSyncPut(kv, 'snippets', ['first'], 0);
    localSyncPut(kv, 'snippets', ['second'], 1);

    const error = thrown(() => localSyncPut(kv, 'snippets', ['stale'], 1));
    expect(error).toBeInstanceOf(RsCoreError);
    expect(error).toMatchObject({
      kind: 'conflict',
      httpStatus: 409,
      hubCode: 'conflict',
      current: { version: 2, value: ['second'] },
    });
    expect(localSyncGet(kv, 'snippets')).toEqual({
      version: 2,
      value: ['second'],
    });
  });

  it('rejects a non-zero version for an empty namespace without current', () => {
    const kv = new MemoryKv();

    const error = thrown(() => localSyncPut(kv, 'networks', [], 3));
    expect(error).toBeInstanceOf(RsCoreError);
    expect(error).toMatchObject({ kind: 'conflict', current: undefined });
    expect(localSyncGet(kv, 'networks')).toBeNull();
  });

  it('keeps namespaces independent', () => {
    const kv = new MemoryKv();
    localSyncPut(kv, 'snippets', ['s'], 0);
    localSyncPut(kv, 'snippets', ['s2'], 1);

    expect(localSyncPut(kv, 'host_notes', { h1: 'n' }, 0)).toEqual({
      version: 1,
      value: { h1: 'n' },
    });
    expect(localSyncGet(kv, 'snippets')).toEqual({
      version: 2,
      value: ['s2'],
    });
    expect(localSyncGet(kv, 'servers')).toBeNull();
  });

  it('treats a malformed stored entry as empty', () => {
    const kv = new MemoryKv({
      'sync.local.settings': JSON.stringify({ value: 1 }),
      'sync.local.networks': '{not json',
    });

    expect(localSyncGet(kv, 'settings')).toBeNull();
    expect(localSyncGet(kv, 'networks')).toBeNull();
    expect(localSyncPut(kv, 'settings', 2, 0)).toEqual({
      version: 1,
      value: 2,
    });
  });
});
