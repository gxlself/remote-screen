import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import type { SyncValue } from '../../native/rscore/types';
import {
  failLocalSync,
  readStaleOnce,
  seedLocalSync,
} from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import { localSyncGet } from '../data/localSync';
import { qk } from '../data/queryClient';
import { MemoryKv } from '../storage/kv';
import { useManualServers } from './useManualServers';
import type { ManualComputer, ManualServer, ServersValue } from './manual';
import { EMPTY_SERVERS } from './manual';

const server: ManualServer = {
  id: 'manual-build-srv',
  kind: 'ssh',
  name: 'build-srv',
  host: '10.30.20.31',
  port: 22,
  user: 'alice',
  auth: 'password',
  jump: 'jump-01',
  networkId: 'n1',
  addedAtUs: FIXTURE_NOW,
  updatedAtUs: FIXTURE_NOW,
};

const computer: ManualComputer = {
  id: 'addr-10-30-12-48-7400',
  kind: 'computer',
  host: '10.30.12.48',
  port: 7400,
  hostId: null,
  name: null,
  networkId: 'n1',
  addedAtUs: FIXTURE_NOW,
};

const clients: QueryClient[] = [];

/** `saved` is the servers namespace already on this phone. */
async function renderManual(saved?: SyncValue) {
  const kv = new MemoryKv();
  if (saved !== undefined) {
    seedLocalSync(kv, 'servers', saved);
  }
  const providers = await renderWithProviders(<></>, { kv });
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(() => useManualServers(), {
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

it('exposes an empty namespace as EMPTY_SERVERS', async () => {
  const { result } = await renderManual();
  await advance();

  expect(result.current.value).toEqual(EMPTY_SERVERS);
  expect(result.current.servers).toEqual([]);
  expect(result.current.computers).toEqual([]);
  expect(result.current.isLoading).toBe(false);
});

it('saves a server and invalidates SSH servers and the directory', async () => {
  const { result, core, kv, queryClient } = await renderManual();
  await advance();
  const invalidate = jest.spyOn(queryClient, 'invalidateQueries');

  await act(async () => {
    await result.current.saveServer(server);
  });
  await advance();

  const expected: ServersValue = {
    v: 1,
    entries: [server],
    tailscaleSsh: {},
  };
  expect(localSyncGet(kv, 'servers')).toEqual({
    version: 1,
    value: expected,
  });
  expect(core.calls).toContainEqual({
    method: 'settings.set',
    args: ['sync.servers', expected],
  });
  expect(result.current.servers).toEqual([server]);
  expect(result.current.value).toEqual(expected);
  expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.sshServers });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.directory });
});

it('merges a concurrent writer on this phone before retrying', async () => {
  const other: ManualComputer = {
    ...computer,
    id: 'addr-192-168-31-20-7400',
    host: '192.168.31.20',
  };
  const initial: ServersValue = {
    v: 1,
    entries: [computer],
    tailscaleSsh: {},
  };
  const current: ServersValue = {
    v: 1,
    entries: [other],
    tailscaleSsh: {},
  };
  const before = { version: 5, value: initial };
  const { result, kv } = await renderManual(before);
  await advance();
  // Another writer on this phone lands after the save read version 5.
  seedLocalSync(kv, 'servers', { version: 6, value: current });
  readStaleOnce(kv, 'servers', before);
  await act(async () => {
    await result.current.saveServer(server);
  });
  await advance();

  expect(result.current.servers).toEqual([server]);
  expect(result.current.computers).toEqual([other]);
  expect(result.current.value.entries).toHaveLength(2);
  expect(result.current.value.entries).toEqual([other, server]);
  expect(localSyncGet(kv, 'servers')?.version).toBe(7);
});

it('removes a computer by id', async () => {
  const { result } = await renderManual({
    version: 2,
    value: { v: 1, entries: [computer] },
  });
  await advance();
  expect(result.current.computers).toHaveLength(1);

  await act(async () => {
    await result.current.removeComputer(computer.id);
  });
  await advance();

  expect(result.current.computers).toEqual([]);
  expect(result.current.value).toEqual({ v: 1, entries: [], tailscaleSsh: {} });
});

it('propagates a failed write to the caller', async () => {
  const error = new Error('disk full');
  const { result, kv } = await renderManual();
  await advance();
  failLocalSync(kv, 'servers', error);

  await act(async () => {
    await expect(result.current.saveServer(server)).rejects.toBe(error);
  });
  expect(result.current.servers).toEqual([]);
});
