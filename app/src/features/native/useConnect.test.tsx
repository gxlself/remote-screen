import { act, cleanup, renderHook } from '@testing-library/react-native';
import { StackActions } from '@react-navigation/native';
import { createElement } from 'react';
import { native } from '../../i18n/zh/native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { computers } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import { installFakeAdapters } from '../adapters/testing/fakeAdapters';
import type { ComputerEntry } from './types';
import { useConnect } from './useConnect';

const mockShowToast = jest.fn();
jest.mock('../../components/Toast', () => ({
  useToast: () => ({ show: mockShowToast }),
}));
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    navigate: jest.fn(),
    dispatch: jest.fn(),
    isReady: jest.fn(() => true),
  },
}));

const navigation = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as { navigate: jest.Mock; dispatch: jest.Mock };

type HostEntry = Extract<ComputerEntry, { kind: 'host' }>;

function hostEntry(
  overrides: Partial<(typeof computers)[number]> = {},
): HostEntry {
  return {
    kind: 'host',
    key: `host:${computers[0].hostId}`,
    computer: {
      ...computers[0],
      ...overrides,
      addresses: overrides.addresses ?? computers[0].addresses,
    },
  };
}

const tailnetEntry: ComputerEntry = {
  kind: 'tailnet',
  key: 'tailnet:n1',
  device: {
    node: 'n1',
    name: 'Tailnet Mac',
    os: 'macos',
    addrs: ['fd7a:115c::7', '100.64.0.7'],
    online: true,
    lastSeenAt: null,
  },
};

async function mounted(opts: Parameters<typeof renderWithProviders>[1] = {}) {
  const core = opts.core ?? createFakeCore();
  const providers = await renderWithProviders(createElement('View'), {
    ...opts,
    core,
  });
  await providers.unmount();
  const hook = await renderHook(() => useConnect(), {
    wrapper: providers.wrapper,
  });
  return {
    ...hook,
    core,
    platform: providers.platform,
    settingsStore: providers.settingsStore,
  };
}

beforeEach(() => {
  navigation.navigate.mockReset();
  navigation.dispatch.mockReset();
  mockShowToast.mockReset();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('routes tailnet Mac entries to the native adapter without probing', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const probe = jest.spyOn(core.discover!, 'probe');
  const view = await mounted({ core });

  await act(async () => view.result.current.connect(tailnetEntry));

  expect(probe).not.toHaveBeenCalled();
  expect(navigation.navigate).toHaveBeenCalledWith('AdapterSession', {
    target: { protocol: 'vnc', addr: '100.64.0.7' },
    name: 'Tailnet Mac',
    addrKind: 'tailnet',
    context: 'native',
  });
});

it('probes host entries and opens the native adapter when the service is open', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const probe = jest.spyOn(core.discover!, 'probe').mockResolvedValue({
    addr: '100.64.0.7',
    port: 5900,
    outcome: 'open',
    ms: 3,
  });
  const view = await mounted({ core });

  await act(async () => view.result.current.connect(hostEntry()));

  expect(probe).toHaveBeenCalledWith('100.64.0.7', 5900, 'vnc', 1500);
  expect(navigation.navigate).toHaveBeenCalledWith('AdapterSession', {
    target: {
      protocol: 'vnc',
      hostId: computers[0].hostId,
      addr: '100.64.0.7',
    },
    name: computers[0].name,
    addrKind: 'tailnet',
    context: 'native',
  });
});

it('falls back to Host and hands the notice to the session page', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  jest.spyOn(core.discover!, 'probe').mockResolvedValue({
    addr: '100.64.0.7',
    port: 5900,
    outcome: 'refused',
    ms: 3,
  });
  const view = await mounted({ core });

  await act(async () => view.result.current.connect(hostEntry()));

  expect(navigation.navigate).toHaveBeenCalledWith('Session', {
    hostId: computers[0].hostId,
    notice: native.toast.fellBack('macos'),
  });
  expect(native.toast.fellBack('macos')).toBe(
    '没打开屏幕共享，已改用 Host 连接',
  );
  // The hook's own toast state is never rendered; the session page shows it.
  expect(mockShowToast).not.toHaveBeenCalled();
});

it('returns the external-app guide when the fallback app is missing', async () => {
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const view = await mounted({ platform });
  expect(view.result.current.guide).toBeNull();

  await act(async () => view.result.current.connect(tailnetEntry));

  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(navigation.dispatch).not.toHaveBeenCalled();
  expect(view.result.current.guide).toEqual({ kind: 'vnc' });

  await act(async () => view.result.current.closeGuide());
  expect(view.result.current.guide).toBeNull();
});

it('replaces the current screen on request and navigates by default', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const view = await mounted({ core });
  const params = {
    target: { protocol: 'vnc', addr: '100.64.0.7' },
    name: 'Tailnet Mac',
    addrKind: 'tailnet',
    context: 'native',
  } as const;

  await act(async () =>
    view.result.current.connect(tailnetEntry, { replace: true }),
  );

  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(navigation.dispatch).toHaveBeenCalledTimes(1);
  const action = navigation.dispatch.mock.calls[0][0];
  expect(action.type).toBe('REPLACE');
  expect(action).toEqual(StackActions.replace('AdapterSession', params));

  await act(async () => view.result.current.connect(tailnetEntry));

  expect(navigation.dispatch).toHaveBeenCalledTimes(1);
  expect(navigation.navigate).toHaveBeenCalledWith('AdapterSession', params);
});

it('shows the sharing guide for a refused service on a down Host', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  jest.spyOn(core.discover!, 'probe').mockResolvedValue({
    addr: '100.64.0.7',
    port: 5900,
    outcome: 'refused',
    ms: 3,
  });
  const view = await mounted({
    core,
  });
  const entry = hostEntry({ online: false, hostRunning: false });

  await act(async () => view.result.current.connect(entry));

  expect(navigation.navigate).toHaveBeenCalledWith('EnableSharing', {
    os: 'macos',
    name: computers[0].name,
    entryKey: `host:${computers[0].hostId}`,
  });
});

it('uses Host preference directly and native preference as an override', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const view = await mounted({ core, settings: { 'connect.prefer': 'host' } });
  const entry = hostEntry();
  const probeSpy = jest.spyOn(core.discover!, 'probe');

  await act(async () => view.result.current.connect(entry));
  expect(probeSpy).not.toHaveBeenCalled();
  expect(navigation.navigate).toHaveBeenCalledWith('Session', {
    hostId: entry.computer.hostId,
  });

  navigation.navigate.mockReset();
  await act(async () => {
    view.settingsStore.setHostPrefs(entry.computer.hostId, {
      connectVia: 'native',
    });
    await Promise.resolve();
  });
  const probe = jest.spyOn(core.discover!, 'probe').mockResolvedValue({
    addr: '100.64.0.7',
    port: 5900,
    outcome: 'open',
    ms: 1,
  });
  await act(async () => view.result.current.connect(entry));
  expect(probe).toHaveBeenCalled();
  expect(navigation.navigate).toHaveBeenCalledWith(
    'AdapterSession',
    expect.objectContaining({ context: 'native' }),
  );
});

it('ignores a second tap while the first connection is probing', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  let resolveProbe:
    | ((value: {
        addr: string;
        port: number;
        outcome: 'open';
        ms: number;
      }) => void)
    | undefined;
  jest.spyOn(core.discover!, 'probe').mockImplementation(
    () =>
      new Promise(resolve => {
        resolveProbe = resolve;
      }),
  );
  const view = await mounted({ core });

  let first: Promise<void> | undefined;
  await act(async () => {
    first = view.result.current.connect(hostEntry());
    await Promise.resolve();
    await view.result.current.connect(hostEntry());
  });
  expect(view.result.current.busy).toBe(true);
  resolveProbe?.({ addr: '100.64.0.7', port: 5900, outcome: 'open', ms: 1 });
  await act(async () => first);
  expect(navigation.navigate).toHaveBeenCalledTimes(1);
});
