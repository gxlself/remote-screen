import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { leaveHubUrl } from '../../test/localDirectory';
import { setSettingsStorage } from '../settings/store';
import { MemoryKv } from '../storage/kv';
import { localSyncGet, localSyncPut } from './localSync';
import { putSyncWithRetry } from './syncRetry';

describe('on this phone', () => {
  let kv: MemoryKv;

  beforeEach(() => {
    kv = new MemoryKv();
    setSettingsStorage(kv);
  });

  const hubCalls = (core: FakeCore) =>
    core.calls.filter(call => call.method.startsWith('hub.'));

  it('writes this phone only, starting from version zero', async () => {
    const core = createFakeCore();
    const update = jest.fn((cur: string[] | null) => [...(cur ?? []), 'mine']);

    await expect(putSyncWithRetry(core, 'snippets', update)).resolves.toEqual({
      version: 1,
      value: ['mine'],
    });
    await expect(putSyncWithRetry(core, 'snippets', update)).resolves.toEqual({
      version: 2,
      value: ['mine', 'mine'],
    });
    expect(update.mock.calls).toEqual([[null], [['mine']]]);
    expect(localSyncGet(kv, 'snippets')).toEqual({
      version: 2,
      value: ['mine', 'mine'],
    });
    expect(hubCalls(core)).toEqual([]);
  });

  it('recomputes once from the local value reported with a conflict', async () => {
    const core = createFakeCore();
    localSyncPut(kv, 'snippets', ['original'], 0);
    const update = jest.fn((cur: string[] | null) => {
      if (update.mock.calls.length === 1) {
        // Another writer on this phone lands between the read and the write.
        localSyncPut(kv, 'snippets', ['other'], 1);
      }
      return [...(cur ?? []), 'mine'];
    });

    await expect(putSyncWithRetry(core, 'snippets', update)).resolves.toEqual({
      version: 3,
      value: ['other', 'mine'],
    });
    expect(update.mock.calls).toEqual([[['original']], [['other']]]);
    expect(hubCalls(core)).toEqual([]);
  });

  it('propagates the second local conflict without a third write', async () => {
    const core = createFakeCore();
    let writes = 0;
    const update = jest.fn((cur: string[] | null) => {
      writes += 1;
      localSyncPut(kv, 'snippets', [`other-${writes}`], writes - 1);
      return [...(cur ?? []), 'mine'];
    });

    await expect(
      putSyncWithRetry(core, 'snippets', update),
    ).rejects.toMatchObject({
      kind: 'conflict',
      current: { version: 2, value: ['other-2'] },
    });
    expect(update.mock.calls).toEqual([[null], [['other-1']]]);
    expect(localSyncGet(kv, 'snippets')).toEqual({
      version: 2,
      value: ['other-2'],
    });
  });

  it('mirrors every successful servers write into Rust', async () => {
    const core = createFakeCore();

    await putSyncWithRetry(core, 'servers', () => ({ items: [1] }));
    await putSyncWithRetry(core, 'snippets', () => ['not servers']);

    expect(
      core.calls.filter(call => call.method === 'settings.set'),
    ).toEqual([{ method: 'settings.set', args: ['sync.servers', { items: [1] }] }]);
  });

  it('keeps the local write when Rust rejects the servers mirror', async () => {
    const core = createFakeCore();
    core.failNext('settings.set', new RsCoreError('unsupported', 'old core'));

    await expect(
      putSyncWithRetry(core, 'servers', () => ({ items: [] })),
    ).resolves.toEqual({ version: 1, value: { items: [] } });
    expect(localSyncGet(kv, 'servers')).toEqual({
      version: 1,
      value: { items: [] },
    });
    expect(core.calls).toContainEqual({
      method: 'logs.write',
      args: ['warn', 'sync', 'sync.servers push failed: old core'],
    });
  });

  it('writes the storage the caller passes instead of the bound one', async () => {
    const core = createFakeCore();
    const own = new MemoryKv();

    await expect(
      putSyncWithRetry(core, 'snippets', () => ['mine'], own),
    ).resolves.toEqual({ version: 1, value: ['mine'] });

    expect(localSyncGet(own, 'snippets')).toEqual({
      version: 1,
      value: ['mine'],
    });
    expect(localSyncGet(kv, 'snippets')).toBeNull();
    expect(hubCalls(core)).toEqual([]);
  });

  it('does not retry a storage error', async () => {
    const core = createFakeCore();
    const error = new Error('disk full');
    jest.spyOn(kv, 'setJSON').mockImplementation(() => {
      throw error;
    });
    const update = jest.fn(() => ['mine']);

    await expect(putSyncWithRetry(core, 'snippets', update)).rejects.toBe(
      error,
    );
    expect(update).toHaveBeenCalledTimes(1);
    expect(hubCalls(core)).toEqual([]);
  });

  it('ignores a Hub address an older version left in MMKV', async () => {
    const core = createFakeCore();
    leaveHubUrl(kv);

    await expect(
      putSyncWithRetry(core, 'servers', () => ({ items: [2] })),
    ).resolves.toEqual({ version: 1, value: { items: [2] } });

    expect(localSyncGet(kv, 'servers')).toEqual({
      version: 1,
      value: { items: [2] },
    });
    expect(
      core.calls.filter(call => call.method === 'settings.set'),
    ).toEqual([{ method: 'settings.set', args: ['sync.servers', { items: [2] }] }]);
    expect(hubCalls(core)).toEqual([]);
  });
});
