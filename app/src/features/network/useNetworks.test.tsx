import {
  act,
  cleanup,
  renderHook,
  waitFor,
} from '@testing-library/react-native';
import type { SyncValue } from '../../native/rscore/types';
import type { NetworkEntry } from '../settings/schema';
import { renderWithProviders } from '../../test/render';
import type { QueryClient } from '@tanstack/react-query';
import type { SettingsStore } from '../settings/store';
import { MemoryKv } from '../storage/kv';
import { localSyncGet } from '../data/localSync';
import { readStaleOnce, seedLocalSync } from '../../test/localDirectory';
import { qk } from '../data/queryClient';
import { useNetworks } from './useNetworks';

const companyNetwork: NetworkEntry = {
  id: 'n1',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

const clients: QueryClient[] = [];

/** `saved` is the networks namespace already on this phone. */
async function renderNetworks(saved?: SyncValue) {
  const kv = new MemoryKv();
  if (saved !== undefined) {
    seedLocalSync(kv, 'networks', saved);
  }
  const providers = await renderWithProviders(<></>, { kv });
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(() => useNetworks(), {
    wrapper: providers.wrapper,
  });
  return {
    ...rendered,
    core: providers.core,
    kv,
    queryClient: providers.queryClient,
    settingsStore: providers.settingsStore,
  };
}

function localNetworks(settingsStore: SettingsStore): NetworkEntry[] {
  return settingsStore.get('networks');
}

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
});

it('starts empty when the namespace has no value', async () => {
  const { result } = await renderNetworks();
  await waitFor(() => expect(result.current.isLoading).toBe(false));
  expect(result.current.items).toEqual([]);
});

it('parses malformed synced values defensively', async () => {
  const { result } = await renderNetworks({
    version: 1,
    value: {
      v: 1,
      items: [
        companyNetwork,
        { name: 'missing id' },
        { id: 'bad', name: 'Bad', cidrs: ['x'], vpnPools: [] },
      ],
    },
  });
  await waitFor(() =>
    expect(result.current.items).toEqual([
      companyNetwork,
      { id: 'bad', name: 'Bad', cidrs: [], vpnPools: [] },
    ]),
  );
});

it('saves on this phone and mirrors the result into local settings', async () => {
  const { result, core, kv, queryClient, settingsStore } =
    await renderNetworks();
  await waitFor(() => expect(result.current.isLoading).toBe(false));

  await act(async () => {
    await result.current.save(companyNetwork);
  });

  expect(localSyncGet(kv, 'networks')).toEqual({
    version: 1,
    value: { v: 1, items: [companyNetwork] },
  });
  expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(false);
  expect(localNetworks(settingsStore)).toEqual([companyNetwork]);
  expect(queryClient.getQueryData(qk.sync('networks'))).toEqual({
    version: 1,
    value: { v: 1, items: [companyNetwork] },
  });
});

it('recomputes once after a sync conflict and persists the merged value', async () => {
  const otherNetwork: NetworkEntry = {
    id: 'n2',
    name: '家里',
    cidrs: ['192.168.31.0/24'],
    vpnPools: [],
  };
  const before = { version: 4, value: { v: 1, items: [otherNetwork] } };
  const { result, kv, settingsStore } = await renderNetworks(before);
  await waitFor(() => expect(result.current.items).toEqual([otherNetwork]));

  const latest = {
    version: 5,
    value: { v: 1, items: [otherNetwork, { ...otherNetwork, id: 'n3' }] },
  };
  // Another writer on this phone lands after the save read version 4.
  seedLocalSync(kv, 'networks', latest);
  readStaleOnce(kv, 'networks', before);

  await act(async () => {
    await result.current.save(companyNetwork);
  });

  expect(localSyncGet(kv, 'networks')).toEqual({
    version: 6,
    value: { v: 1, items: [...latest.value.items, companyNetwork] },
  });
  expect(localNetworks(settingsStore)).toEqual([
    ...latest.value.items,
    companyNetwork,
  ]);
});

it('removes an entry and clears the local mirror', async () => {
  const { result, kv, settingsStore } = await renderNetworks({
    version: 2,
    value: { v: 1, items: [companyNetwork] },
  });
  await waitFor(() => expect(result.current.items).toEqual([companyNetwork]));

  await act(async () => {
    await result.current.remove('n1');
  });

  expect(localSyncGet(kv, 'networks')).toEqual({
    version: 3,
    value: { v: 1, items: [] },
  });
  expect(localNetworks(settingsStore)).toEqual([]);
});
