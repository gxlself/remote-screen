import type { ReactNode } from 'react';
import { act, render, renderHook, screen } from '@testing-library/react-native';
import { Text } from 'react-native';
import { CoreProvider } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  liveSession,
  netCompanyVpn,
  netTailnetOk,
} from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import { PATHS_KEY } from '../directory/pathPersist';
import { StorageProvider } from '../storage/StorageContext';
import { MemoryKv } from '../storage/kv';
import { createEventBus } from './bus';
import EventsProvider, {
  selectLive,
  useCoreEvent,
  useHostPath,
  useNetwork,
  useSessions,
  useSessionsActions,
} from './EventsProvider';

function wrapper(core: ReturnType<typeof createFakeCore>) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>
      <EventsProvider>{children}</EventsProvider>
    </CoreProvider>
  );
}

function storedWrapper(core: ReturnType<typeof createFakeCore>, kv: MemoryKv) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>
      <StorageProvider kv={kv}>
        <EventsProvider>{children}</EventsProvider>
      </StorageProvider>
    </CoreProvider>
  );
}

const prewarmResult: CoreEventOf<'probe.result'> = {
  type: 'probe.result',
  probeId: 'prewarm',
  hostId: liveSession.hostId,
  path: 'lan',
  addr: '192.168.1.20',
  port: 7400,
  transport: 'quic',
  outcome: 'ok',
  ms: 9,
  last: true,
};

const stats: CoreEventOf<'session.stats'> = {
  type: 'session.stats',
  sessionId: liveSession.id,
  rttMs: 6,
  fps: 60,
  bitrateKbps: 8000,
  decodeMs: 3,
  lossPct: 0,
  width: 2560,
  height: 1600,
  codec: 'hevc',
  path: 'intranet',
  transport: 'quic',
};

it('dispatches in order and isolates throwing listeners', () => {
  const bus = createEventBus();
  const order: string[] = [];
  const error = jest.spyOn(console, 'error').mockImplementation(() => {});

  bus.on('session.stats', () => order.push('first'));
  bus.on('session.stats', () => {
    throw new Error('listener failed');
  });
  bus.on('*', () => order.push('all'));
  bus.on('session.stats', () => order.push('last'));

  bus.emit(stats);
  expect(order).toEqual(['first', 'all', 'last']);
  expect(error).toHaveBeenCalledTimes(1);
  error.mockRestore();

  bus.emit({
    type: 'pairing.done',
    outcome: { status: 'cancelled' },
  });
  expect(order).toEqual(['first', 'all', 'last', 'all']);
});

it('forwards only matching events and unsubscribes on unmount', async () => {
  const core = createFakeCore();
  const handler = jest.fn();

  function Probe() {
    useCoreEvent('session.stats', handler);
    return null;
  }

  const view = await render(
    <CoreProvider core={core}>
      <EventsProvider>
        <Probe />
      </EventsProvider>
    </CoreProvider>,
  );
  expect(core.subscriberCount()).toBe(1);

  await act(() => core.emit(stats));
  expect(handler).toHaveBeenCalledWith(stats);

  await act(() =>
    core.emit({
      type: 'session.clipboard',
      sessionId: liveSession.id,
      direction: 'fromHost',
      text: 'hello',
      at: 1,
    }),
  );
  expect(handler).toHaveBeenCalledTimes(1);

  await view.unmount();
  expect(core.subscriberCount()).toBe(0);
});

it('subscribes once to the core event stream', async () => {
  const core = createFakeCore();
  await render(
    <CoreProvider core={core}>
      <EventsProvider>
        <Text>ready</Text>
      </EventsProvider>
    </CoreProvider>,
  );
  expect(screen.getByText('ready')).toBeOnTheScreen();
  expect(
    core.calls.filter(call => call.method === 'events.subscribe'),
  ).toHaveLength(1);
});

it('seeds live sessions from the core', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(() => useSessions(selectLive), {
    wrapper: wrapper(core),
  });
  expect(result.current.map(item => item.id)).toEqual([liveSession.id]);
});

it('applies session events to the store and UI subscribers', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(() => useSessions(selectLive), {
    wrapper: wrapper(core),
  });

  await act(() =>
    core.emit({
      type: 'session.state',
      sessionId: liveSession.id,
      hostId: liveSession.hostId,
      phase: 'reconnecting',
      reconnectReason: 'pathLost',
    }),
  );
  expect(result.current[0]?.phase).toBe('reconnecting');
  expect(result.current[0]?.reconnectReason).toBe('pathLost');
});

it('exposes session actions that clear prompts', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(
    () => ({
      sessions: useSessions(state => state[liveSession.id]),
      actions: useSessionsActions(),
    }),
    { wrapper: wrapper(core) },
  );

  await act(() =>
    core.emit({
      type: 'session.idleWarning',
      sessionId: liveSession.id,
      secondsLeft: 30,
      lockOnEnd: false,
    }),
  );
  expect(result.current.sessions?.idleWarning?.secondsLeft).toBe(30);

  await act(() => result.current.actions.clearIdleWarning(liveSession.id));
  expect(result.current.sessions?.idleWarning).toBeNull();
});

it('uses network.changed as the latest network status', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(() => useNetwork(state => state.status), {
    wrapper: wrapper(core),
  });

  await act(() =>
    core.emit({ type: 'network.changed', status: netCompanyVpn }),
  );
  expect(result.current).toEqual(netCompanyVpn);
});

it('reads the initial network status', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(() => useNetwork(state => state.status), {
    wrapper: wrapper(core),
  });
  expect(result.current).toEqual(netTailnetOk);
});

it('links session stats to the directory path store', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(() => useHostPath(liveSession.hostId), {
    wrapper: wrapper(core),
  });

  await act(() => core.emit(stats));
  expect(result.current).toMatchObject({
    path: 'intranet',
    latencyMs: 6,
    transport: 'quic',
  });
});

it('keeps network status null when the initial read is unsupported', async () => {
  const core = createFakeCore();
  core.failNext('network.status', new RsCoreError('unsupported', 'x'));

  const { result } = await renderHook(
    () => ({
      status: useNetwork(state => state.status),
      updatedAtUs: useNetwork(state => state.updatedAtUs),
    }),
    { wrapper: wrapper(core) },
  );
  expect(result.current.status).toBeNull();
  expect(result.current.updatedAtUs).toBeNull();
});

it('keeps provider state isolated between mounts', async () => {
  const firstCore = createFakeCore();
  const secondCore = createFakeCore();
  const first = await renderHook(() => useSessions(selectLive), {
    wrapper: wrapper(firstCore),
  });

  await act(() =>
    firstCore.emit({
      type: 'session.error',
      sessionId: liveSession.id,
      hostId: liveSession.hostId,
      code: 140,
      reason: 'userClosed',
    }),
  );
  expect(first.result.current).toEqual([]);

  const second = await renderHook(() => useSessions(selectLive), {
    wrapper: wrapper(secondCore),
  });
  expect(second.result.current).toHaveLength(1);
});

it('renders without a CoreProvider so the U0 app shell can mount', async () => {
  await render(
    <EventsProvider>
      <Text>shell</Text>
    </EventsProvider>,
  );
  expect(screen.getByText('shell')).toBeOnTheScreen();
});

it('restores persisted paths before the first render', async () => {
  const core = createFakeCore();
  const kv = new MemoryKv();
  const atUs = Date.now() * 1000 - 3_600_000_000;
  kv.setJSON(PATHS_KEY, {
    [liveSession.hostId]: {
      path: 'tailnetDirect',
      latencyMs: 31,
      transport: 'quic',
      atUs,
    },
  });
  const { result } = await renderHook(() => useHostPath(liveSession.hostId), {
    wrapper: storedWrapper(core, kv),
  });
  expect(result.current).toEqual({
    path: 'tailnetDirect',
    latencyMs: 31,
    transport: 'quic',
    atUs,
  });
});

it('saves a prewarm result once the save delay has passed', async () => {
  jest.useFakeTimers();
  try {
    const core = createFakeCore();
    const kv = new MemoryKv();
    const { result } = await renderHook(() => useHostPath(liveSession.hostId), {
      wrapper: storedWrapper(core, kv),
    });

    await act(() => core.emit(prewarmResult));
    expect(result.current).toMatchObject({
      path: 'lan',
      latencyMs: 9,
      transport: 'quic',
    });
    expect(kv.getString(PATHS_KEY)).toBeUndefined();

    await act(() => jest.advanceTimersByTime(2_000));
    expect(kv.getJSON(PATHS_KEY)).toEqual({
      [liveSession.hostId]: result.current,
    });
  } finally {
    jest.useRealTimers();
  }
});

it('keeps paths in memory without a StorageProvider', async () => {
  const core = createFakeCore();
  const { result } = await renderHook(() => useHostPath(liveSession.hostId), {
    wrapper: wrapper(core),
  });
  await act(() => core.emit(prewarmResult));
  expect(result.current).toMatchObject({ path: 'lan', latencyMs: 9 });
});
