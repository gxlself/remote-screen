import { act, cleanup, render } from '@testing-library/react-native';
import type { ReactNode } from 'react';
import { getLaunchOptions, setLaunchOptions } from '../../app/launchOptions';
import { CoreProvider } from '../../native/rscore/CoreContext';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import EventsProvider from '../events/EventsProvider';
import { useLockState } from '../lock/lockState';
import { MemoryKv } from '../storage/kv';
import { SettingsProvider, createSettingsStore } from '../settings/store';
import { harnessLog } from '../../native/devharness/log';
import { leaveHubUrl } from '../../test/localDirectory';
import AutoConnect from './AutoConnect';

type NavigationListener = () => void;

const mockNavigationListeners = new Map<string, NavigationListener>();

jest.mock('../../app/navigation/navigationRef', () => {
  const navigationRef = {
    addListener: jest.fn(),
    getCurrentRoute: jest.fn(() => ({ name: 'MainTabs' })),
    isReady: jest.fn(() => true),
    navigate: jest.fn(),
  };
  return { navigationRef };
});

jest.mock('../../native/devharness/log', () => ({ harnessLog: jest.fn() }));

const mockNavigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as {
  addListener: jest.Mock;
  getCurrentRoute: jest.Mock;
  isReady: jest.Mock;
  navigate: jest.Mock;
};
const mockLog = jest.mocked(harnessLog);
const originalOptions = getLaunchOptions();

function setAutoConnect(autoConnect: string, disconnectAfterS?: number): void {
  setLaunchOptions({
    ...originalOptions,
    autoConnect,
    ...(disconnectAfterS === undefined ? {} : { disconnectAfterS }),
  });
}

function providers(
  core: FakeCore,
  onboardingDone: boolean,
  leftoverHubUrl = false,
  tailnetEnabled = false,
) {
  const kv = new MemoryKv({
    ...(onboardingDone ? { 'settings.onboarding.done': 'true' } : {}),
    ...(tailnetEnabled ? { 'settings.tailnet.enabled': 'true' } : {}),
  });
  if (leftoverHubUrl) {
    leaveHubUrl(kv);
  }
  const settingsStore = createSettingsStore(kv, core);
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <CoreProvider core={core}>
        <SettingsProvider store={settingsStore}>
          <EventsProvider>{children}</EventsProvider>
        </SettingsProvider>
      </CoreProvider>
    );
  }
  return { Wrapper, settingsStore };
}

async function settle() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function emitState(
  core: FakeCore,
  phase: 'streaming' | 'ended',
  sessionId: string,
  hostId: string,
) {
  return act(() =>
    core.emit({
      type: 'session.state',
      sessionId,
      hostId,
      phase,
    }),
  );
}

function emitStats(core: FakeCore, sessionId: string, rttMs: number) {
  return act(() =>
    core.emit({
      type: 'session.stats',
      sessionId,
      rttMs,
      fps: 60,
      bitrateKbps: 8000,
      decodeMs: 3,
      lossPct: 0,
      width: 2560,
      height: 1600,
      codec: 'hevc',
      path: 'tailnetDirect',
      transport: 'quic',
    }),
  );
}

function logged(event: string) {
  return mockLog.mock.calls
    .map(([, payload]) => payload)
    .find(payload => payload.event === event);
}

beforeEach(() => {
  jest.useFakeTimers();
  mockNavigationListeners.clear();
  mockNavigationRef.addListener.mockImplementation(
    (event: string, listener: NavigationListener) => {
      mockNavigationListeners.set(event, listener);
      return () => mockNavigationListeners.delete(event);
    },
  );
  mockNavigationRef.getCurrentRoute.mockReturnValue({ name: 'MainTabs' });
  mockNavigationRef.isReady.mockReturnValue(true);
  mockNavigationRef.navigate.mockClear();
  mockLog.mockClear();
  useLockState.setState({ locked: false });
  setLaunchOptions(originalOptions);
});

afterEach(async () => {
  await cleanup();
  setLaunchOptions(originalOptions);
  jest.useRealTimers();
});

it('waits for onboarding, reports it once, then navigates', async () => {
  setAutoConnect('100.64.0.7');
  const core = createFakeCore();
  const { Wrapper, settingsStore } = providers(core, false);
  await render(<AutoConnect />, {
    wrapper: Wrapper,
  });
  await settle();

  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
  await act(() => jest.advanceTimersByTimeAsync(3000));
  expect(logged('waiting')).toEqual({ event: 'waiting', reason: 'onboarding' });

  await act(async () => settingsStore.set('onboarding.done', true));
  await settle();
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: 'h7v2m9q3k8x4tb1a',
  });
});

it('navigates after onboarding completes', async () => {
  setAutoConnect('100.64.0.7');
  const core = createFakeCore();
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();

  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: 'h7v2m9q3k8x4tb1a',
  });
});

it('resolves a paired address to its host ID', async () => {
  setAutoConnect('100.64.0.7:9000');
  const core = createFakeCore();
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();

  expect(logged('resolve')).toEqual({
    event: 'resolve',
    kind: 'addr',
    hostId: 'h7v2m9q3k8x4tb1a',
    ambiguous: false,
  });
});

it('reports an unknown address and stays on Home', async () => {
  setAutoConnect('100.64.0.99');
  const core = createFakeCore();
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();

  expect(logged('error')).toEqual({
    event: 'error',
    error: 'unknownHost',
  });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});

it('reads only the local directory for an address no Host has', async () => {
  setAutoConnect('100.64.0.18');
  const core = createFakeCore();
  await render(<AutoConnect />, {
    wrapper: providers(core, true, false, true).Wrapper,
  });
  await settle();

  const methods = core.calls.map(call => call.method);
  expect(methods.filter(method => method === 'hosts.paired')).toHaveLength(2);
  // The in-app tailnet is off, so its node list is not asked for.
  expect(methods).toContain('tailnet.available');
  expect(methods).not.toContain('tailnet.status');
  expect(methods.filter(method => method.startsWith('hub.'))).toEqual([]);
  expect(logged('error')).toEqual({ event: 'error', error: 'unknownHost' });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});

it('never asks the tailnet while Tailscale is switched off', async () => {
  setAutoConnect('100.64.0.18');
  const core = createFakeCore();
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();

  const methods = core.calls.map(call => call.method);
  expect(methods).toContain('hosts.paired');
  expect(methods).not.toContain('tailnet.available');
  expect(methods).not.toContain('tailnet.status');
  expect(methods).not.toContain('tailscaleApi.status');
  expect(logged('error')).toEqual({ event: 'error', error: 'unknownHost' });
});

it('resolves a paired address despite a Hub address left in MMKV', async () => {
  setAutoConnect('100.64.0.7');
  const core = createFakeCore();
  await render(<AutoConnect />, {
    wrapper: providers(core, true, true).Wrapper,
  });
  await settle();

  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: 'h7v2m9q3k8x4tb1a',
  });
  expect(
    core.calls.filter(call => call.method.startsWith('hub.')),
  ).toEqual([]);
});

it('closes after the configured delay and summarizes collected samples', async () => {
  setAutoConnect('k3jq7x2m9p4tbw8a', 5);
  const core = createFakeCore();
  const close = jest.spyOn(core.sessions, 'close');
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();

  await emitState(core, 'streaming', 's-test', 'k3jq7x2m9p4tbw8a');
  await emitStats(core, 's-test', 4);
  await emitStats(core, 's-test', 6);
  await emitStats(core, 's-test', 8);
  await act(() => jest.advanceTimersByTimeAsync(5000));

  expect(close).toHaveBeenCalledWith('s-test');
  expect(logged('summary')).toEqual({
    event: 'summary',
    sessionId: 's-test',
    durationS: 5,
    samples: 3,
    rttP50: 6,
    rttP95: 8,
    fpsAvg: 60,
    decodeP50: 3,
    decodeP95: 3,
  });
});

it('navigates only once when the navigation state changes again', async () => {
  setAutoConnect('100.64.0.7');
  const core = createFakeCore();
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();
  await act(() => mockNavigationListeners.get('state')?.());
  await settle();

  expect(mockNavigationRef.navigate).toHaveBeenCalledTimes(1);
});

it('logs unsupported pointer movement two seconds after streaming', async () => {
  setLaunchOptions({
    ...originalOptions,
    autoConnect: 'k3jq7x2m9p4tbw8a',
    selfTestMove: true,
  });
  const core = createFakeCore();
  await render(<AutoConnect />, { wrapper: providers(core, true).Wrapper });
  await settle();
  await emitState(core, 'streaming', liveSession.id, 'k3jq7x2m9p4tbw8a');
  await act(() => jest.advanceTimersByTimeAsync(2000));

  expect(logged('selfTestMove')).toEqual({
    event: 'selfTestMove',
    status: 'unsupported',
  });
});
