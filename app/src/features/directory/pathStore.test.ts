import type { CoreEvent } from '../../native/rscore/types';
import { applyPathEvent, createPathStore } from './pathStore';
import type { PathState } from './pathStore';

const HOST = 'h1';
const SESSION = 's1';
const NOW = 1_800_000_000_000_000;

const empty: PathState = { byHost: {}, routes: {} };
const ctx = {
  hostIdOf: (sessionId: string) => (sessionId === SESSION ? HOST : undefined),
  nowUs: NOW,
};

const stats: CoreEvent = {
  type: 'session.stats',
  sessionId: SESSION,
  rttMs: 6,
  fps: 60,
  bitrateKbps: 4000,
  decodeMs: 3,
  lossPct: 0,
  width: 2560,
  height: 1600,
  codec: 'hevc',
  path: 'intranet',
  transport: 'quic',
  derpRegion: 'Shanghai',
};

function probe(
  addr: string,
  ms: number | null,
  outcome: 'ok' | 'timeout',
  path: 'intranet' | 'tailnetDerp' = 'intranet',
): CoreEvent {
  return {
    type: 'probe.result',
    probeId: `${addr}-${ms}`,
    hostId: HOST,
    path,
    addr,
    port: 7400,
    transport: 'quic',
    outcome,
    ms,
    last: true,
  };
}

it('records session latency per host', () => {
  const state = applyPathEvent(empty, stats, ctx);
  expect(state.byHost[HOST]).toMatchObject({
    path: 'intranet',
    latencyMs: 6,
    transport: 'quic',
    derpRegion: 'Shanghai',
    atUs: NOW,
  });
});

it('ignores stats when the session host cannot be resolved', () => {
  const state = applyPathEvent(empty, stats, {
    hostIdOf: () => undefined,
    nowUs: NOW,
  });
  expect(state).toBe(empty);
});

it('stores every probe and chooses the fastest successful route', () => {
  const intranet = probe('10.30.1.20', 23, 'ok');
  const derp = probe('100.64.0.7', 48, 'ok', 'tailnetDerp');
  const afterIntranet = applyPathEvent(empty, intranet, ctx);
  const both = applyPathEvent(afterIntranet, derp, ctx);

  expect(Object.keys(both.routes[HOST] ?? {})).toEqual([
    '10.30.1.20:7400',
    '100.64.0.7:7400',
  ]);
  expect(both.routes[HOST]?.['10.30.1.20:7400']).toMatchObject({
    outcome: 'ok',
    ms: 23,
    atUs: NOW,
  });
  expect(both.byHost[HOST].latencyMs).toBe(23);
});

it('keeps failed probes out of the host latency', () => {
  const state = applyPathEvent(
    empty,
    probe('10.30.1.21', null, 'timeout'),
    ctx,
  );
  expect(state.routes[HOST]?.['10.30.1.21:7400']?.outcome).toBe('timeout');
  expect(state.byHost[HOST]).toBeUndefined();
});

it('does not replace a recent faster latency with a slower probe', () => {
  const initial: PathState = {
    byHost: {
      [HOST]: {
        path: 'lan',
        latencyMs: 10,
        transport: 'quic',
        atUs: NOW - 30_000_000,
      },
    },
    routes: {},
  };
  const state = applyPathEvent(initial, probe('10.30.1.22', 23, 'ok'), ctx);
  expect(state.byHost[HOST].latencyMs).toBe(10);
});

it('replaces latency older than sixty seconds', () => {
  const initial: PathState = {
    byHost: {
      [HOST]: {
        path: 'lan',
        latencyMs: 10,
        transport: 'quic',
        atUs: NOW - 61_000_000,
      },
    },
    routes: {},
  };
  const state = applyPathEvent(initial, probe('10.30.1.23', 23, 'ok'), ctx);
  expect(state.byHost[HOST].latencyMs).toBe(23);
});

it('returns the same state for unrelated or hostless events', () => {
  const unrelated: CoreEvent = {
    type: 'pairing.done',
    outcome: { status: 'cancelled' },
  };
  expect(applyPathEvent(empty, unrelated, ctx)).toBe(empty);

  const hostless = {
    ...probe('10.30.1.24', 8, 'ok'),
    hostId: null,
  } as CoreEvent;
  expect(applyPathEvent(empty, hostless, ctx)).toBe(empty);
});

it('starts from the given initial state', () => {
  const byHost: PathState['byHost'] = {
    [HOST]: { path: 'lan', latencyMs: 10, transport: 'quic', atUs: NOW },
  };
  expect(createPathStore({ byHost }).getState()).toEqual({
    byHost,
    routes: {},
  });
  expect(createPathStore().getState()).toEqual(empty);
});

it('lets an ok probe replace a restored path older than sixty seconds', () => {
  const restored = (ageUs: number) =>
    createPathStore({
      byHost: {
        [HOST]: {
          path: 'lan',
          latencyMs: 10,
          transport: 'quic',
          atUs: NOW - ageUs,
        },
      },
    });
  const slower = probe('10.30.1.25', 23, 'ok');

  const recent = restored(59_000_000);
  recent.setState(state => applyPathEvent(state, slower, ctx));
  expect(recent.getState().byHost[HOST]).toMatchObject({
    path: 'lan',
    latencyMs: 10,
  });

  const old = restored(61_000_000);
  old.setState(state => applyPathEvent(state, slower, ctx));
  expect(old.getState().byHost[HOST]).toEqual({
    path: 'intranet',
    latencyMs: 23,
    transport: 'quic',
    atUs: NOW,
  });
});
