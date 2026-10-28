import type { ReactNode } from 'react';
import { act, render, waitFor } from '@testing-library/react-native';
import { CoreProvider } from '../rscore/CoreContext';
import { createFakeCore } from '../rscore/fakeCore';
import { RsCoreError } from '../rscore/RsCoreApi';
import { MemoryKv } from '../../features/storage/kv';
import { SettingsProvider, createSettingsStore } from '../../features/settings/store';
import NetInfoBridge from './NetInfoBridge';
import { nativeNetworkStore } from './nativeNetworkStore';
import type { NativeNetStatus } from '../specs/NativeNetInfoPlus';

const mockGetStatus = jest.fn<Promise<NativeNetStatus>, []>();
const mockSetVpnPools = jest.fn<void, [string[]]>();
const mockOnChange = jest.fn<{
  remove: () => void;
}, [(status: NativeNetStatus) => void]>();
let mockChange: ((status: NativeNetStatus) => void) | undefined;
let mockNativeModule: {
  getStatus: () => Promise<NativeNetStatus>;
  setVpnPools: (pools: string[]) => void;
  onChange: (
    listener: (status: NativeNetStatus) => void,
  ) => { remove: () => void };
} | null = null;

jest.mock('../specs/NativeNetInfoPlus', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

const nativeStatus: NativeNetStatus = {
  ifaces: [{ name: 'en0', addrs: ['192.168.31.20/24'], isVpn: false }],
  path: {
    status: 'satisfied',
    interfaceTypes: ['wifi'],
    expensive: false,
    constrained: false,
  },
  seq: 1,
};

function wrapper(core: ReturnType<typeof createFakeCore>, settings: ReturnType<typeof createSettingsStore>) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>
      <SettingsProvider store={settings}>{children}</SettingsProvider>
    </CoreProvider>
  );
}

beforeEach(() => {
  mockGetStatus.mockResolvedValue(nativeStatus);
  mockSetVpnPools.mockClear();
  mockOnChange.mockClear();
  mockOnChange.mockImplementation(() => ({ remove: jest.fn() }));
  mockChange = undefined;
  mockNativeModule = {
    getStatus: (...args: []) => mockGetStatus(...args),
    setVpnPools: (...args: [string[]]) => mockSetVpnPools(...args),
    onChange: (...args: [(status: NativeNetStatus) => void]) => {
      mockChange = args[0];
      return mockOnChange(args[0]);
    },
  };
  nativeNetworkStore.setState({ native: null, core: null });
});

it('deduplicates VPN pools and refreshes native and core status on changes', async () => {
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv());
  settings.set('networks', [
    { id: 'a', name: 'A', cidrs: [], vpnPools: ['10.99.0.0/16', '10.30.0.0/16'] },
    { id: 'b', name: 'B', cidrs: [], vpnPools: ['10.99.0.0/16'] },
  ]);
  await render(<NetInfoBridge />, { wrapper: wrapper(core, settings) });

  await waitFor(() => expect(mockSetVpnPools).toHaveBeenCalledWith(['10.99.0.0/16', '10.30.0.0/16']));
  expect(nativeNetworkStore.getState().native).toEqual(nativeStatus);

  await act(() => {
    settings.set('networks', [
      { id: 'a', name: 'A', cidrs: [], vpnPools: ['10.99.0.0/16'] },
    ]);
  });
  await waitFor(() => expect(mockSetVpnPools).toHaveBeenCalledTimes(2));

  expect(mockChange).toBeDefined();
  await act(async () => {
    mockChange?.({ ...nativeStatus, seq: 2 });
  });
  await waitFor(() => expect(core.calls.some(call => call.method === 'network.status')).toBe(true));
  expect(nativeNetworkStore.getState().native?.seq).toBe(2);
  expect(nativeNetworkStore.getState().core).not.toBeNull();
  expect(
    core.calls.filter(
      call =>
        call.method === 'settings.set' &&
        call.args[0] === 'device.workProfileIfaces',
    ),
  ).toHaveLength(0);
});

it('pushes work-profile interface names before refreshing core status', async () => {
  const status: NativeNetStatus = {
    ...nativeStatus,
    ifaces: [
      { name: 'tun2', addrs: [], isVpn: true, workProfile: true },
      { name: 'tun1', addrs: [], isVpn: true, workProfile: true },
      { name: 'tun1', addrs: [], isVpn: true, workProfile: true },
    ],
  };
  mockGetStatus.mockResolvedValue(status);
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv());
  await render(<NetInfoBridge />, { wrapper: wrapper(core, settings) });
  await waitFor(() =>
    expect(nativeNetworkStore.getState().native).toEqual(status),
  );

  await act(async () => {
    mockChange?.({ ...status, seq: 2 });
  });
  await waitFor(() =>
    expect(core.calls).toContainEqual({
      method: 'settings.set',
      args: ['device.workProfileIfaces', ['tun1', 'tun2']],
    }),
  );
  const settingsIndex = core.calls.findIndex(
    call =>
      call.method === 'settings.set' &&
      call.args[0] === 'device.workProfileIfaces',
  );
  const statusIndex = core.calls.findIndex(
    call => call.method === 'network.status',
  );
  expect(settingsIndex).toBeGreaterThanOrEqual(0);
  expect(statusIndex).toBeGreaterThan(settingsIndex);
});

it('does not repeat an unchanged work-profile set and clears it when the interface disappears', async () => {
  const status: NativeNetStatus = {
    ...nativeStatus,
    ifaces: [{ name: 'tun1', addrs: [], isVpn: true, workProfile: true }],
  };
  mockGetStatus.mockResolvedValue(status);
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv());
  await render(<NetInfoBridge />, { wrapper: wrapper(core, settings) });
  await waitFor(() =>
    expect(nativeNetworkStore.getState().native).toEqual(status),
  );

  await act(async () => {
    mockChange?.({ ...status, seq: 2 });
    mockChange?.({
      ...status,
      seq: 3,
      ifaces: [{ name: 'tun1', addrs: [], isVpn: true }],
    });
  });
  await waitFor(() =>
    expect(
      core.calls.filter(
        call =>
          call.method === 'settings.set' &&
          call.args[0] === 'device.workProfileIfaces',
      ),
    ).toEqual([
      { method: 'settings.set', args: ['device.workProfileIfaces', ['tun1']] },
      { method: 'settings.set', args: ['device.workProfileIfaces', []] },
    ]),
  );
  expect(nativeNetworkStore.getState().native?.seq).toBe(3);
});

it('stops work-profile pushes after an unsupported setting while keeping native updates', async () => {
  const status: NativeNetStatus = {
    ...nativeStatus,
    ifaces: [{ name: 'tun1', addrs: [], isVpn: true, workProfile: true }],
  };
  mockGetStatus.mockResolvedValue(status);
  const warn = jest.spyOn(console, 'warn').mockImplementation(() => {});
  const core = createFakeCore();
  core.failNext('settings.set', new RsCoreError('unsupported', 'x'));
  const settings = createSettingsStore(new MemoryKv());
  await render(<NetInfoBridge />, { wrapper: wrapper(core, settings) });
  await waitFor(() =>
    expect(nativeNetworkStore.getState().native).toEqual(status),
  );
  expect(warn).toHaveBeenCalledTimes(1);

  await act(async () => {
    mockChange?.({ ...status, seq: 2 });
  });
  expect(
    core.calls.filter(
      call =>
        call.method === 'settings.set' &&
        call.args[0] === 'device.workProfileIfaces',
    ),
  ).toHaveLength(1);
  expect(nativeNetworkStore.getState().native?.seq).toBe(2);
});

it('clears the core value after an unsupported status call without retrying', async () => {
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv());
  await render(<NetInfoBridge />, { wrapper: wrapper(core, settings) });
  await waitFor(() => expect(nativeNetworkStore.getState().native).toEqual(nativeStatus));

  core.failNext('network.status', new RsCoreError('unsupported', 'x'));
  await act(async () => {
    mockChange?.({ ...nativeStatus, seq: 2 });
  });
  await waitFor(() => expect(nativeNetworkStore.getState().core).toBeNull());

  await act(async () => {
    mockChange?.({ ...nativeStatus, seq: 3 });
  });
  expect(core.calls.filter(call => call.method === 'network.status')).toHaveLength(1);
});

it('mounts safely when the native module is unavailable', async () => {
  mockNativeModule = null;
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv());
  expect(() => render(<NetInfoBridge />, { wrapper: wrapper(core, settings) })).not.toThrow();
});
