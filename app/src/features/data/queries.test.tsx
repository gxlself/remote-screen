import { act, cleanup, renderHook } from '@testing-library/react-native';
import { focusManager } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { SELF_DEVICE_ID } from '../../native/rscore/fixtures';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { MemoryKv } from '../storage/kv';
import { leaveHubUrl } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import { createQueryClient, qk, setupAppFocus } from './queryClient';
import {
  useDirectory,
  useIdentity,
  usePairedHosts,
  usePutSync,
  useSshKeys,
  useSshServers,
  useSync,
  useTrustedHostKeys,
} from './queries';

const clients: QueryClient[] = [];
type ProviderOptions = Parameters<typeof renderWithProviders>[1];

async function renderQueryHook<T>(hook: () => T, opts?: ProviderOptions) {
  const providers = await renderWithProviders(<></>, opts);
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(hook, { wrapper: providers.wrapper });
  return {
    core: providers.core,
    platform: providers.platform,
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

beforeEach(() => {
  jest.useFakeTimers();
  focusManager.setFocused(true);
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  focusManager.setFocused(undefined);
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('sets production query defaults', () => {
  const client = createQueryClient();
  expect(client.getDefaultOptions().queries).toMatchObject({
    retry: 1,
    staleTime: 0,
  });
  client.clear();
});

it('drives focus from AppState and removes the listener', () => {
  let onChange: ((state: AppStateStatus) => void) | undefined;
  const remove = jest.fn();
  jest
    .spyOn(AppState, 'addEventListener')
    .mockImplementation((_event, listener) => {
      onChange = listener;
      return { remove };
    });
  const previousState = AppState.currentState;
  AppState.currentState = 'background';
  try {
    const stop = setupAppFocus();
    expect(focusManager.isFocused()).toBe(false);
    onChange?.('active');
    expect(focusManager.isFocused()).toBe(true);
    onChange?.('inactive');
    expect(focusManager.isFocused()).toBe(false);
    stop();
    expect(remove).toHaveBeenCalledTimes(1);
  } finally {
    AppState.currentState = previousState;
  }
});

it('reads the paired computers on this phone, not the Hub', async () => {
  const { result, core } = await renderQueryHook(useDirectory, {
    localDirectory: {},
  });
  await advance();
  expect(result.current.directory?.computers).toHaveLength(4);
  expect(result.current.isLoading).toBe(false);
  expect(result.current.error).toBeNull();
  expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(false);
});

it('returns no directory when the paired computers cannot be read', async () => {
  const core = createFakeCore();
  const error = new RsCoreError('internal', 'disk');
  core.failNext('hosts.paired', error);
  const { result } = await renderQueryHook(useDirectory, { core });
  await advance();
  expect(result.current.directory).toBeUndefined();
  expect(result.current.error).toBe(error);
});

it('keeps the directory fresh for five seconds and polls only in the foreground', async () => {
  const { core, queryClient } = await renderQueryHook(useDirectory);
  const calls = () =>
    core.calls.filter(call => call.method === 'hosts.paired').length;
  const query = queryClient
    .getQueryCache()
    .find({ queryKey: qk.directory, exact: false });
  expect(query?.options).toMatchObject({
    staleTime: 5000,
    refetchInterval: 10000,
    refetchIntervalInBackground: false,
  });
  await advance();
  expect(calls()).toBe(1);
  await advance(9997);
  expect(calls()).toBe(1);
  await advance(4);
  expect(calls()).toBe(2);
  await act(() => focusManager.setFocused(false));
  await advance(20000);
  expect(calls()).toBe(2);
  await act(() => focusManager.setFocused(true));
  await advance();
  expect(calls()).toBe(3);
});

it('shares one identity request between two mounted components', async () => {
  const core = createFakeCore();
  const platform = new FakePlatform();
  platform.info.model = 'Test tablet';
  const gate: { resolve?: () => void } = {};
  const pending = new Promise<void>(resolve => {
    gate.resolve = resolve;
  });
  const ensure = core.identity.ensure;
  const request = jest
    .spyOn(core.identity, 'ensure')
    .mockImplementation(async model => {
      await pending;
      return ensure(model);
    });
  const providers = await renderWithProviders(<></>, { core, platform });
  clients.push(providers.queryClient);
  await providers.unmount();
  const first = await renderHook(useIdentity, { wrapper: providers.wrapper });
  const second = await renderHook(useIdentity, { wrapper: providers.wrapper });
  expect(request).toHaveBeenCalledTimes(1);
  expect(first.result.current.data).toBeUndefined();
  expect(second.result.current.data).toBeUndefined();
  await act(() => gate.resolve?.());
  await advance();
  expect(first.result.current.data?.deviceId).toBe(SELF_DEVICE_ID);
  expect(second.result.current.data?.deviceId).toBe(SELF_DEVICE_ID);
  expect(core.calls.filter(call => call.method === 'identity.ensure')).toEqual([
    { method: 'identity.ensure', args: ['Test tablet'] },
  ]);
});

it('loads paired hosts and SSH lists with their freshness periods', async () => {
  const { result, queryClient } = await renderQueryHook(() => ({
    paired: usePairedHosts(),
    servers: useSshServers(),
    keys: useSshKeys(),
    trusted: useTrustedHostKeys(),
  }));
  await advance();
  for (const value of Object.values(result.current)) {
    expect(value.isSuccess).toBe(true);
  }
  for (const [key, staleTime] of [
    [qk.pairedHosts, 60000],
    [qk.sshServers, 30000],
    [qk.sshKeys, 30000],
    [qk.trustedHostKeys, 30000],
  ] as const) {
    expect(
      queryClient.getQueryCache().find({ queryKey: key })?.options,
    ).toMatchObject({ staleTime });
  }
});

it('updates the sync query from a successful mutation', async () => {
  const { result, queryClient } = await renderQueryHook(() => ({
    sync: useSync<string[]>('servers'),
    put: usePutSync<string[]>('servers'),
  }));
  await advance();
  expect(result.current.sync.data).toBeNull();
  await act(async () => {
    await result.current.put.mutateAsync(cur => [...(cur ?? []), 'mine']);
  });
  await advance();
  const expected = { version: 1, value: ['mine'] };
  expect(queryClient.getQueryData(qk.sync('servers'))).toEqual(expected);
  expect(result.current.sync.data).toEqual(expected);
});

it('asks the Hub nothing with a Hub address an older version left in MMKV', async () => {
  const { result, core } = await renderQueryHook(
    () => ({
      paired: usePairedHosts(),
      identity: useIdentity(),
      servers: useSshServers(),
      keys: useSshKeys(),
      trusted: useTrustedHostKeys(),
    }),
    { kv: leaveHubUrl(new MemoryKv()) },
  );
  await advance(2000);
  for (const value of Object.values(result.current)) {
    expect(value.isSuccess).toBe(true);
  }
  expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual([]);
});
