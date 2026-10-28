import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { SyncValue } from '../../native/rscore/types';
import { readStaleOnce, seedLocalSync } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import { localSyncGet } from '../data/localSync';
import { qk } from '../data/queryClient';
import { MemoryKv } from '../storage/kv';
import { useSnippets } from './useSnippets';
import type { Snippet, SnippetsValue } from './snippets';

const initial: Snippet[] = [
  { id: 'docker', label: '', cmd: 'docker ps', run: true },
  { id: 'tail', label: '', cmd: 'tail -f api.log', run: true },
  { id: 'systemctl', label: '', cmd: 'systemctl status api', run: false },
];

const initialValue: SnippetsValue = { v: 1, items: initial };

const clients: QueryClient[] = [];

/** `saved` is the snippets namespace already on this phone. */
async function renderSnippets(saved: SyncValue) {
  const kv = new MemoryKv();
  seedLocalSync(kv, 'snippets', saved);
  const providers = await renderWithProviders(<></>, { kv });
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(() => useSnippets(), {
    wrapper: providers.wrapper,
  });
  return {
    ...rendered,
    core: providers.core,
    kv,
    queryClient: providers.queryClient,
  };
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('loads the three preset snippets', async () => {
  const { result } = await renderSnippets({
    version: 1,
    value: initialValue,
  });
  await advance();

  expect(result.current.items).toEqual(initial);
  expect(result.current.items).toHaveLength(3);
  expect(result.current.isLoading).toBe(false);
});

it('adds a snippet through the versioned sync write', async () => {
  const { result, kv } = await renderSnippets({
    version: 1,
    value: initialValue,
  });
  await advance();

  await act(async () => {
    await result.current.add({ label: '', cmd: 'htop', run: true });
  });
  await advance();

  expect(localSyncGet(kv, 'snippets')).toEqual({
    version: 2,
    value: {
      v: 1,
      items: [...initial, expect.objectContaining({ cmd: 'htop', run: true })],
    },
  });
  expect(result.current.items).toHaveLength(4);
  expect(result.current.items[3]).toMatchObject({
    label: '',
    cmd: 'htop',
    run: true,
  });
  expect(result.current.items[3].id).toMatch(/^sn-[a-z0-9]+$/);
});

it('removes the first snippet and keeps the remaining order', async () => {
  const { result, kv } = await renderSnippets({
    version: 1,
    value: initialValue,
  });
  await advance();

  await act(async () => {
    await result.current.add({ label: '', cmd: 'htop', run: true });
  });
  await advance();
  expect(result.current.items).toHaveLength(4);

  await act(async () => {
    await result.current.remove('docker');
  });
  await advance();

  expect(result.current.items).toHaveLength(3);
  expect(result.current.items[0].cmd).toBe('tail -f api.log');
  expect(localSyncGet(kv, 'snippets')).toEqual({
    version: 3,
    value: {
      v: 1,
      items: [...initial.slice(1), expect.objectContaining({ cmd: 'htop' })],
    },
  });
});

it('merges one conflict from the current value on this phone', async () => {
  const other: Snippet = {
    id: 'other',
    label: 'logs',
    cmd: 'journalctl -u api',
    run: false,
  };
  const current: SnippetsValue = { v: 1, items: [other] };
  const before = { version: 5, value: { v: 1, items: [initial[0]] } };
  const { result, kv } = await renderSnippets(before);
  await advance();
  // Another writer on this phone lands after the add read version 5.
  seedLocalSync(kv, 'snippets', { version: 6, value: current });
  readStaleOnce(kv, 'snippets', before);
  await act(async () => {
    await result.current.add({ label: '', cmd: 'htop', run: true });
  });
  await advance();

  expect(result.current.items.map(item => item.id)).toEqual([
    'other',
    expect.stringMatching(/^sn-/),
  ]);
  expect(result.current.items[1].cmd).toBe('htop');
  expect(localSyncGet(kv, 'snippets')?.version).toBe(7);
});

it('updates the cached sync query after a successful write', async () => {
  const { result, queryClient } = await renderSnippets({
    version: 1,
    value: initialValue,
  });
  await advance();
  const updated = { ...initial[0], label: 'containers' };

  await act(async () => {
    await result.current.update(updated);
  });
  await advance();

  expect(result.current.items[0]).toEqual(updated);
  expect(queryClient.getQueryData(qk.sync('snippets'))).toEqual({
    version: 2,
    value: { v: 1, items: [updated, initial[1], initial[2]] },
  });
});
