import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { leaveHubUrl } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import { MemoryKv } from '../storage/kv';
import type { KvStore } from '../storage/kv';
import { localSyncGet, localSyncPut } from './localSync';
import { qk } from './queryClient';
import { usePutSync, useSync } from './syncQuery';

const clients: QueryClient[] = [];

async function renderSync(
  core: FakeCore = createFakeCore(),
  kv: KvStore = new MemoryKv(),
) {
  const providers = await renderWithProviders(<></>, { core, kv });
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(
    () => ({
      sync: useSync<string[]>('servers'),
      put: usePutSync<string[]>('servers'),
    }),
    { wrapper: providers.wrapper },
  );
  return {
    kv: providers.kv,
    settingsStore: providers.settingsStore,
    queryClient: providers.queryClient,
    ...rendered,
  };
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

const hubCalls = (core: FakeCore) =>
  core.calls.filter(call => call.method.startsWith('hub.'));
const serversPushes = (core: FakeCore) =>
  core.calls.filter(
    call => call.method === 'settings.set' && call.args[0] === 'sync.servers',
  );

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.useRealTimers();
});

describe('on this phone', () => {
  it('reads and writes this phone without calling the Hub', async () => {
    const core = createFakeCore();
    const { result, kv, queryClient } = await renderSync(core);
    await advance();
    expect(result.current.sync.data).toBeNull();

    await act(async () => {
      await result.current.put.mutateAsync(cur => [...(cur ?? []), 'mine']);
    });
    await advance();

    const expected = { version: 1, value: ['mine'] };
    expect(localSyncGet(kv, 'servers')).toEqual(expected);
    expect(queryClient.getQueryData(qk.sync('servers'))).toEqual(expected);
    expect(result.current.sync.data).toEqual(expected);
    expect(hubCalls(core)).toEqual([]);
    expect(serversPushes(core)).toEqual([
      { method: 'settings.set', args: ['sync.servers', ['mine']] },
    ]);
  });

  it('serves an existing local value under the same query key', async () => {
    const providers = await renderWithProviders(<></>);
    clients.push(providers.queryClient);
    localSyncPut(providers.kv, 'snippets', ['ls'], 0);
    await providers.unmount();
    const { result } = await renderHook(() => useSync<string[]>('snippets'), {
      wrapper: providers.wrapper,
    });
    await advance();

    expect(result.current.data).toEqual({ version: 1, value: ['ls'] });
    expect(
      providers.queryClient.getQueryData(['sync', 'snippets']),
    ).toEqual({ version: 1, value: ['ls'] });
    expect(hubCalls(providers.core)).toEqual([]);
  });

  it('retries a local conflict once and then succeeds', async () => {
    const { result, kv } = await renderSync();
    await advance();
    const update = jest.fn((cur: string[] | null) => {
      if (update.mock.calls.length === 1) {
        // Another writer on this phone lands between the read and the write.
        localSyncPut(kv, 'servers', ['other'], 0);
      }
      return [...(cur ?? []), 'mine'];
    });

    await act(async () => {
      await result.current.put.mutateAsync(update);
    });
    await advance();

    expect(update.mock.calls).toEqual([[null], [['other']]]);
    expect(result.current.sync.data).toEqual({
      version: 2,
      value: ['other', 'mine'],
    });
  });

  it('ignores a Hub address an older version left in MMKV', async () => {
    const core = createFakeCore();
    const kv = leaveHubUrl(new MemoryKv());
    localSyncPut(kv, 'servers', ['saved'], 0);
    const { result } = await renderSync(core, kv);
    await advance();
    expect(result.current.sync.data).toEqual({ version: 1, value: ['saved'] });

    await act(async () => {
      await result.current.put.mutateAsync(cur => [...(cur ?? []), 'mine']);
    });
    await advance();

    const expected = { version: 2, value: ['saved', 'mine'] };
    expect(localSyncGet(kv, 'servers')).toEqual(expected);
    expect(result.current.sync.data).toEqual(expected);
    expect(hubCalls(core)).toEqual([]);
    expect(serversPushes(core)).toEqual([
      { method: 'settings.set', args: ['sync.servers', ['saved', 'mine']] },
    ]);
  });
});
