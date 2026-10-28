import {
  act,
  cleanup,
  renderHook,
  waitFor,
} from '@testing-library/react-native';
import type { ReactNode } from 'react';
import type { QueryClient } from '@tanstack/react-query';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { computers } from '../../native/rscore/fixtures';
import type { Computer } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { useEndActions } from './useEndActions';

const STUDIO = 'h7v2m9q3k8x4tb1a';
const OFFICE = 'w3k8p2x7m4q9tb6c';

const queryClients: QueryClient[] = [];

afterEach(async () => {
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
});

async function renderActions(
  host: Computer | undefined,
  platform = new FakePlatform(),
) {
  const navigation = mockNavigation<'SessionEnded'>();
  const providers = await renderWithProviders(<></>, { platform });
  queryClients.push(providers.queryClient);

  function Wrapper({ children }: { children: ReactNode }) {
    return providers.wrapper({ children });
  }

  const hook = await renderHook(
    () =>
      useEndActions({
        route: mockRoute('SessionEnded', { hostId: host?.hostId ?? STUDIO }),
        navigation: navigation as never,
        host,
      }),
    { wrapper: Wrapper },
  );
  return { ...hook, navigation, platform };
}

it('retries the session host and offers no wake action', async () => {
  const host = computers.find(computer => computer.hostId === STUDIO);
  const { result, navigation } = await renderActions(host);

  result.current.retry();

  expect(navigation.replace).toHaveBeenCalledWith('Session', {
    hostId: STUDIO,
  });
  expect(Object.keys(result.current)).not.toContain('wake');
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('checks and opens the RDP URL with port 3389 for Windows', async () => {
  const platform = new FakePlatform();
  const host = computers.find(computer => computer.hostId === OFFICE);
  const { result } = await renderActions(host, platform);

  await act(async () => {
    await result.current.openScreenSharing();
  });

  expect(platform.calls).toEqual([
    {
      method: 'canOpenUrl',
      args: ['rdp://full%20address=s:100.64.0.12:3389'],
    },
    {
      method: 'openUrl',
      args: ['rdp://full%20address=s:100.64.0.12:3389'],
    },
  ]);
});

it('brackets an IPv6 tailnet address for a Mac', async () => {
  const platform = new FakePlatform();
  const host: Computer = {
    ...(computers.find(computer => computer.hostId === STUDIO) as Computer),
    addresses: [{ kind: 'tailnet', addr: 'fd7a:115c::7', port: 7400 }],
  };
  const { result } = await renderActions(host, platform);

  await act(async () => {
    await result.current.openScreenSharing();
  });

  expect(platform.calls).toEqual([
    { method: 'canOpenUrl', args: ['vnc://[fd7a:115c::7]'] },
    { method: 'openUrl', args: ['vnc://[fd7a:115c::7]'] },
  ]);
});

it('shows a guide and no toast when the VNC app is missing', async () => {
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const host = computers.find(computer => computer.hostId === STUDIO);
  const { result } = await renderActions(host, platform);

  await act(async () => {
    await result.current.openScreenSharing();
  });

  expect(result.current.guide).toEqual({ kind: 'vnc' });
  expect(result.current.toastState).toBeNull();
  expect(platform.calls.map(call => call.method)).toEqual(['canOpenUrl']);
});

it('toasts when the host has no usable address', async () => {
  const platform = new FakePlatform();
  const host: Computer = {
    ...(computers.find(computer => computer.hostId === STUDIO) as Computer),
    addresses: [],
  };
  const { result } = await renderActions(host, platform);

  await act(async () => {
    await result.current.openScreenSharing();
  });

  // V32: the reason is the missing address, not a missing app.
  await waitFor(() => {
    expect(result.current.toastState?.message).toBe(
      '不知道这台电脑的地址，没法直接打开屏幕共享',
    );
  });
});

it('names both tools when the computer is unknown (V32, V60)', async () => {
  const { result } = await renderActions(undefined);

  await act(async () => {
    await result.current.openScreenSharing();
  });

  await waitFor(() => {
    expect(result.current.toastState?.message).toBe(
      '不知道这台电脑的地址，没法直接打开屏幕共享或远程桌面',
    );
  });
});

it('says remote desktop did not open for Windows (V32)', async () => {
  const platform = new FakePlatform();
  jest.spyOn(platform, 'openUrl').mockResolvedValue(false);
  const host = computers.find(computer => computer.hostId === OFFICE);
  const { result } = await renderActions(host, platform);

  await act(async () => {
    await result.current.openScreenSharing();
  });

  await waitFor(() => {
    expect(result.current.toastState?.message).toBe(
      '没能打开远程桌面，再试一次',
    );
  });
  expect(result.current.guide).toBeNull();
});

it('replaces the end page for every way out except diagnostics', async () => {
  const host = computers.find(computer => computer.hostId === STUDIO);
  const { result, navigation } = await renderActions(host);

  await act(async () => {
    result.current.retry();
    result.current.openSession(OFFICE);
    result.current.openTerminal('prod-api-01');
    result.current.pair('scan');
  });

  expect(navigation.replace).toHaveBeenCalledWith('Terminal', {
    serverId: 'prod-api-01',
  });
  expect(navigation.replace).toHaveBeenCalledTimes(4);
  expect(navigation.navigate).not.toHaveBeenCalled();
});
