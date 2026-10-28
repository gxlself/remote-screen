import { MemoryKv } from '../storage/kv';
import { PATHS_KEY, attachPathPersistence, loadPaths } from './pathPersist';
import { createPathStore } from './pathStore';
import type { HostPath, RouteResult } from './pathStore';

const NOW = 1_800_000_000_000_000;
const HOUR_US = 3_600_000_000;

function hostPath(atUs: number, latencyMs = 12): HostPath {
  return { path: 'lan', latencyMs, transport: 'quic', atUs };
}

const route: RouteResult = {
  path: 'lan',
  addr: '192.168.1.20',
  port: 7400,
  transport: 'quic',
  outcome: 'timeout',
  ms: null,
  atUs: NOW,
};

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  jest.useRealTimers();
});

it('writes byHost only once the two second delay has passed', () => {
  const kv = new MemoryKv();
  const store = createPathStore();
  const detach = attachPathPersistence(store, kv);

  store.setState({ byHost: { h1: hostPath(NOW) } });
  jest.advanceTimersByTime(1_999);
  expect(kv.getString(PATHS_KEY)).toBeUndefined();
  jest.advanceTimersByTime(1);
  expect(kv.getJSON(PATHS_KEY)).toEqual({ h1: hostPath(NOW) });
  detach();
});

it('coalesces changes within the delay into one write', () => {
  const kv = new MemoryKv();
  const set = jest.spyOn(kv, 'set');
  const store = createPathStore();
  const detach = attachPathPersistence(store, kv);

  store.setState({ byHost: { h1: hostPath(NOW) } });
  jest.advanceTimersByTime(1_500);
  store.setState({ byHost: { h1: hostPath(NOW + 1, 8) } });
  jest.advanceTimersByTime(500);
  expect(set).toHaveBeenCalledTimes(1);
  expect(kv.getJSON(PATHS_KEY)).toEqual({ h1: hostPath(NOW + 1, 8) });
  detach();
});

it('never writes routes', () => {
  const kv = new MemoryKv();
  const store = createPathStore();
  const detach = attachPathPersistence(store, kv);

  store.setState({ routes: { h1: { '192.168.1.20:7400': route } } });
  jest.advanceTimersByTime(2_000);
  expect(kv.keys()).toEqual([]);

  store.setState({ byHost: { h1: hostPath(NOW) } });
  jest.advanceTimersByTime(2_000);
  expect(kv.keys()).toEqual([PATHS_KEY]);
  expect(kv.getJSON(PATHS_KEY)).toEqual({ h1: hostPath(NOW) });
  expect(kv.getString(PATHS_KEY)).not.toContain('192.168.1.20');
  detach();
});

it('keeps only the 200 most recently updated hosts', () => {
  const kv = new MemoryKv();
  const byHost: Record<string, HostPath> = {};
  // Insertion order is the reverse of recency: h0 is the oldest.
  for (let i = 0; i < 250; i += 1) {
    byHost[`h${i}`] = hostPath(NOW - (249 - i) * 1_000_000);
  }
  const store = createPathStore();
  const detach = attachPathPersistence(store, kv);

  store.setState({ byHost });
  jest.advanceTimersByTime(2_000);
  const saved = kv.getJSON<Record<string, HostPath>>(PATHS_KEY) ?? {};
  expect(Object.keys(saved)).toHaveLength(200);
  expect(saved.h49).toBeUndefined();
  expect(saved.h50).toEqual(byHost.h50);
  expect(saved.h249).toEqual(byHost.h249);
  expect(Object.keys(store.getState().byHost)).toHaveLength(250);
  detach();
});

it('flushes a pending write on detach and stops saving afterwards', () => {
  const kv = new MemoryKv();
  const store = createPathStore();
  const detach = attachPathPersistence(store, kv);

  store.setState({ byHost: { h1: hostPath(NOW) } });
  detach();
  expect(kv.getJSON(PATHS_KEY)).toEqual({ h1: hostPath(NOW) });

  store.setState({ byHost: { h2: hostPath(NOW) } });
  jest.advanceTimersByTime(2_000);
  expect(kv.getJSON(PATHS_KEY)).toEqual({ h1: hostPath(NOW) });
});

it('loads well-formed entries from the last 24 hours', () => {
  const kv = new MemoryKv();
  const derp: HostPath = {
    path: 'tailnetDerp',
    latencyMs: 40,
    transport: 'tcp',
    derpRegion: 'Shanghai',
    atUs: NOW - 23 * HOUR_US,
  };
  kv.setJSON(PATHS_KEY, {
    derp: { ...derp, extra: 'dropped' },
    edge: hostPath(NOW - 24 * HOUR_US),
    stale: hostPath(NOW - 24 * HOUR_US - 1),
    badPath: { ...hostPath(NOW), path: 'carrierPigeon' },
    badLatency: { ...hostPath(NOW), latencyMs: '12' },
    badTransport: { ...hostPath(NOW), transport: 'udp' },
    badAt: { ...hostPath(NOW), atUs: null },
    badRegion: { ...hostPath(NOW), derpRegion: 7 },
    missing: { path: 'lan', transport: 'quic', atUs: NOW },
    text: 'lan',
    empty: null,
  });

  expect(loadPaths(kv, NOW)).toEqual({
    derp,
    edge: hostPath(NOW - 24 * HOUR_US),
  });
});

it('loads nothing from a missing, corrupt or non-object value', () => {
  expect(loadPaths(new MemoryKv(), NOW)).toEqual({});
  for (const raw of ['{not json', '[]', '42', 'null']) {
    expect(loadPaths(new MemoryKv({ [PATHS_KEY]: raw }), NOW)).toEqual({});
  }
});
