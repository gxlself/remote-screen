import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { ReactNode } from 'react';
import { Platform } from 'react-native';
import { StackActions } from '@react-navigation/native';
import { navigationRef } from '../../app/navigation/navigationRef';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { installFakeAdapters } from '../adapters/testing/fakeAdapters';
import { renderWithProviders } from '../../test/render';
import { useRemoteFallback } from './useRemoteFallback';

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

async function renderFallback(platform: FakePlatform, withAdapters = false) {
  const core = createFakeCore();
  const adapters = withAdapters ? installFakeAdapters(core) : undefined;
  const providers = await renderWithProviders(<></>, { core, platform });
  function Wrapper({ children }: { children: ReactNode }) {
    return providers.wrapper({ children });
  }
  const hook = await renderHook(() => useRemoteFallback(), {
    wrapper: Wrapper,
  });
  return { ...hook, core, platform, adapters };
}

it('guides to the store without opening when no VNC app is installed', async () => {
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const { result, core } = await renderFallback(platform);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({ os: 'macos', addr: 'fd7a:115c::7' });
  });

  expect(outcome).toBe('notInstalled');
  expect(platform.calls.map(call => call.method)).toEqual(['canOpenUrl']);
  expect(platform.calls[0].args).toEqual(['vnc://[fd7a:115c::7]']);
  expect(result.current.guide).toEqual({ kind: 'vnc' });
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['info', 'fallback', '{"kind":"vnc","result":"notInstalled"}'],
  });
  expect(JSON.stringify(core.calls)).not.toContain('fd7a:115c::7');
});

it('opens the RDP URL after the protocol check succeeds', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'windows',
      addr: '100.64.0.9',
      port: 3389,
    });
  });

  expect(outcome).toBe('opened');
  expect(platform.calls).toEqual([
    { method: 'canOpenUrl', args: ['rdp://full%20address=s:100.64.0.9:3389'] },
    { method: 'openUrl', args: ['rdp://full%20address=s:100.64.0.9:3389'] },
  ]);
  expect(result.current.guide).toBeNull();
});

it('reports a failed open without showing a guide', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform);
  jest.spyOn(platform, 'canOpenUrl').mockResolvedValue(true);
  platform.openUrlResult = false;

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({ os: 'macos', addr: 'fd7a:115c::7' });
  });
  expect(outcome).toBe('failed');
  expect(result.current.guide).toBeNull();
  expect(platform.calls.map(call => call.method)).toEqual(['openUrl']);
});

it('closes the guide through closeGuide', async () => {
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  const { result } = await renderFallback(platform);

  await act(async () => {
    await result.current.open({ os: 'windows', addr: '100.64.0.9' });
  });
  expect(result.current.guide).toEqual({ kind: 'rdp' });

  await act(async () => result.current.closeGuide());
  expect(result.current.guide).toBeNull();
});

it('opens the RDP URL directly on Android without a protocol check', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const platform = new FakePlatform();
  jest.spyOn(platform, 'canOpenUrl');
  const { result } = await renderFallback(platform);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'windows',
      addr: '100.64.0.9',
      port: 3389,
    });
  });

  expect(outcome).toBe('opened');
  expect(platform.calls).toEqual([
    { method: 'openUrl', args: ['rdp://full%20address=s:100.64.0.9:3389'] },
  ]);
  expect(platform.canOpenUrl).not.toHaveBeenCalled();
  expect(result.current.guide).toBeNull();
});

it('treats a rejected Android open as missing and shows the VNC guide', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const platform = new FakePlatform();
  platform.openUrlResult = false;
  jest.spyOn(platform, 'canOpenUrl');
  const { result, core } = await renderFallback(platform);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({ os: 'macos', addr: 'fd7a:115c::7' });
  });

  expect(outcome).toBe('notInstalled');
  expect(result.current.guide).toEqual({ kind: 'vnc' });
  expect(platform.calls.map(call => call.method)).toEqual(['openUrl']);
  expect(platform.canOpenUrl).not.toHaveBeenCalled();
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['info', 'fallback', '{"kind":"vnc","result":"notInstalled"}'],
  });
  expect(JSON.stringify(core.calls)).not.toContain('fd7a:115c::7');
});

it('opens a built-in VNC adapter when it is available', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'macos',
      addr: '100.64.0.7',
      hostId: 'h1',
      name: 'Mac mini',
      addrKind: 'tailnet',
      context: 'loginWindow',
    });
  });

  expect(outcome).toBe('adapter');
  expect(navigate).toHaveBeenCalledWith('AdapterSession', {
    target: { protocol: 'vnc', hostId: 'h1', addr: '100.64.0.7' },
    name: 'Mac mini',
    addrKind: 'tailnet',
    context: 'loginWindow',
  });
  expect(platform.calls).toEqual([]);
});

it('opens a built-in RDP adapter for a probed Windows address', async () => {
  const platform = new FakePlatform();
  const { result, core } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'windows',
      addr: '192.168.31.20',
      port: 3389,
    });
  });

  expect(outcome).toBe('adapter');
  expect(navigate).toHaveBeenCalledWith('AdapterSession', {
    target: { protocol: 'rdp', addr: '192.168.31.20' },
    addrKind: 'manual',
    context: 'manual',
  });
  expect(platform.calls).toEqual([]);
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['info', 'fallback', '{"kind":"rdp","result":"adapter"}'],
  });
});

it('keeps a non-default RDP port for a known Windows host', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'windows',
      addr: '100.64.0.12',
      port: 3390,
      hostId: 'w1',
      name: 'Office PC',
      addrKind: 'tailnet',
    });
  });

  expect(outcome).toBe('adapter');
  expect(navigate).toHaveBeenCalledWith('AdapterSession', {
    target: { protocol: 'rdp', hostId: 'w1', addr: '100.64.0.12', port: 3390 },
    name: 'Office PC',
    addrKind: 'tailnet',
    context: 'fallback',
  });
  expect(platform.calls).toEqual([]);
});

it('keeps RDP on the external app when adapters are unavailable', async () => {
  const platform = new FakePlatform();
  const { result, adapters } = await renderFallback(platform, true);
  adapters?.setAvailable(false);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'windows',
      addr: '192.168.31.20',
      port: 3389,
    });
  });

  expect(outcome).toBe('opened');
  expect(platform.opened).toEqual([
    'rdp://full%20address=s:192.168.31.20:3389',
  ]);
  expect(navigate).not.toHaveBeenCalled();
});

it('uses the external RDP app when explicitly preferred', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'windows',
      addr: '10.30.12.48',
      hostId: 'w1',
      preferExternal: true,
    });
  });

  expect(outcome).toBe('opened');
  expect(platform.opened).toEqual(['rdp://full%20address=s:10.30.12.48:3389']);
  expect(navigate).not.toHaveBeenCalled();
});

it('uses the external app when explicitly preferred', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);

  let outcome: string | null = null;
  await act(async () => {
    outcome = await result.current.open({
      os: 'macos',
      addr: '100.64.0.7',
      hostId: 'h1',
      preferExternal: true,
    });
  });

  expect(outcome).toBe('opened');
  expect(platform.opened).toEqual(['vnc://100.64.0.7']);
  expect(navigate).not.toHaveBeenCalled();
});

it('falls back externally when adapters are unavailable or navigation is not ready', async () => {
  const unavailablePlatform = new FakePlatform();
  const unavailable = await renderFallback(unavailablePlatform, true);
  unavailable.adapters?.setAvailable(false);
  const isReady = jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  jest.spyOn(navigationRef, 'navigate').mockImplementation(() => undefined);

  await act(async () => {
    await unavailable.result.current.open({ os: 'macos', addr: '100.64.0.7' });
  });
  expect(unavailablePlatform.opened).toEqual(['vnc://100.64.0.7']);

  const oldPlatform = new FakePlatform();
  const oldPackage = await renderFallback(oldPlatform);
  await act(async () => {
    await oldPackage.result.current.open({ os: 'macos', addr: '100.64.0.7' });
  });
  expect(oldPlatform.opened).toEqual(['vnc://100.64.0.7']);

  const notReadyPlatform = new FakePlatform();
  const notReady = await renderFallback(notReadyPlatform, true);
  isReady.mockReturnValue(false);
  await act(async () => {
    await notReady.result.current.open({ os: 'macos', addr: '100.64.0.7' });
  });
  expect(notReadyPlatform.opened).toEqual(['vnc://100.64.0.7']);
});

it('replaces the current page when asked to (V06)', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);
  const dispatch = jest
    .spyOn(navigationRef, 'dispatch')
    .mockImplementation(() => undefined);

  await act(async () => {
    await result.current.open({
      os: 'macos',
      addr: '100.64.0.7',
      hostId: 'h1',
      name: 'Mac mini',
      replace: true,
    });
  });

  expect(navigate).not.toHaveBeenCalled();
  expect(dispatch).toHaveBeenCalledWith(
    StackActions.replace('AdapterSession', {
      target: { protocol: 'vnc', hostId: 'h1', addr: '100.64.0.7' },
      name: 'Mac mini',
      context: 'fallback',
    }),
  );
});

it('replaces the ended-session page so back skips the stale error (V06)', async () => {
  const platform = new FakePlatform();
  const { result } = await renderFallback(platform, true);
  jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  jest.spyOn(navigationRef, 'current', 'get').mockReturnValue({
    getCurrentRoute: () => ({ key: 'ended-1', name: 'SessionEnded' }),
  } as unknown as typeof navigationRef.current);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);
  const dispatch = jest
    .spyOn(navigationRef, 'dispatch')
    .mockImplementation(() => undefined);

  await act(async () => {
    await result.current.open({ os: 'windows', addr: '100.64.0.9' });
  });

  expect(navigate).not.toHaveBeenCalled();
  expect(dispatch).toHaveBeenCalledTimes(1);
  expect(dispatch.mock.calls[0]?.[0]).toMatchObject({
    type: 'REPLACE',
    payload: { name: 'AdapterSession' },
  });
});
