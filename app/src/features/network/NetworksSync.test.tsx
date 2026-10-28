import { act, cleanup } from '@testing-library/react-native';
import { focusManager } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import type { SyncValue } from '../../native/rscore/types';
import { LOCAL_SYNC_PREFIX } from '../data/localSync';
import type { NetworkEntry } from '../settings/schema';
import type { SettingsStore } from '../settings/store';
import { MemoryKv } from '../storage/kv';
import { renderWithProviders } from '../../test/render';
import NetworksSync from './NetworksSync';

const companyNetwork: NetworkEntry = {
  id: 'n1',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

const homeNetwork: NetworkEntry = {
  id: 'home',
  name: '家里',
  cidrs: ['192.168.31.0/24'],
  vpnPools: [],
};

const clients: QueryClient[] = [];

const NETWORKS_KEY = `${LOCAL_SYNC_PREFIX}networks`;

async function renderSync(opts: {
  /** The synced networks namespace saved on this phone. */
  saved?: SyncValue;
  local?: NetworkEntry[];
  failGet?: boolean;
}) {
  const kv = new MemoryKv();
  if (opts.saved !== undefined) {
    kv.setJSON(NETWORKS_KEY, opts.saved);
  }
  if (opts.failGet === true) {
    const getJSON = kv.getJSON.bind(kv);
    jest.spyOn(kv, 'getJSON').mockImplementation((key: string) => {
      if (key === NETWORKS_KEY) {
        throw new Error('storage unavailable');
      }
      return getJSON(key);
    });
  }
  const providers = await renderWithProviders(<NetworksSync />, {
    kv,
    settings: { networks: opts.local ?? [] },
  });
  clients.push(providers.queryClient);
  return providers;
}

function localNetworks(settingsStore: SettingsStore): NetworkEntry[] {
  return settingsStore.get('networks');
}

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  focusManager.setFocused(undefined);
  jest.restoreAllMocks();
});

async function flush(ms = 0): Promise<void> {
  await act(async () => {
    await new Promise<void>(resolve => setTimeout(resolve, ms));
  });
}

it('copies two synced networks into local settings', async () => {
  const { settingsStore } = await renderSync({
    saved: {
      version: 3,
      value: { v: 1, items: [companyNetwork, homeNetwork] },
    },
  });

  await flush();
  expect(localNetworks(settingsStore)).toEqual([companyNetwork, homeNetwork]);
});

it('keeps a local value when nothing is synced yet', async () => {
  const { settingsStore } = await renderSync({ local: [companyNetwork] });

  await flush();
  expect(settingsStore.get('networks')).toEqual([companyNetwork]);
});

it('keeps a local value when the synced value itself is null', async () => {
  const { settingsStore } = await renderSync({
    local: [companyNetwork],
    saved: { version: 1, value: null },
  });

  await flush();
  expect(settingsStore.get('networks')).toEqual([companyNetwork]);
});

it('does not overwrite local settings when the query fails', async () => {
  const { settingsStore } = await renderSync({
    local: [companyNetwork],
    failGet: true,
  });

  await flush();
  expect(settingsStore.get('networks')).toEqual([companyNetwork]);
});
