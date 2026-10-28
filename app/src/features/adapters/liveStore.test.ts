import { ADAPTER_RS_E } from '../../native/rscore/types';
import type { AdapterTarget } from '../../native/rscore/types';
import {
  adapterLive,
  applyAdapterLiveEvent,
  findCollapsed,
  resetAdapterLive,
  resumeKind,
  routeParamsOf,
  sameTarget,
  selectCollapsed,
} from './liveStore';
import type { CollapsedAdapter } from './liveStore';
import { LOST_IN_BACKGROUND } from './logic';

const vnc: AdapterTarget = { protocol: 'vnc', hostId: 'h1', addr: '100.64.0.7' };

function record(
  sessionId: string,
  overrides: Partial<CollapsedAdapter> = {},
): CollapsedAdapter {
  return {
    sessionId,
    params: { target: vnc },
    name: 'Mac mini',
    phase: 'streaming',
    error: null,
    screens: [],
    screenIndex: 0,
    mode: 'trackpad',
    startedAtUs: 1,
    collapsedAtUs: 2,
    history: {
      live: null,
      stoppedAtUs: null,
      error: null,
      name: 'Mac mini',
      targetId: 'host:h1',
    },
    ...overrides,
  };
}

afterEach(() => {
  resetAdapterLive();
});

it('take hands the record back once', () => {
  adapterLive.put(record('a'));
  expect(adapterLive.take('a')?.sessionId).toBe('a');
  expect(adapterLive.take('a')).toBeUndefined();
  expect(adapterLive.getState()).toEqual({});
});

it('decides how a return picks a record up', () => {
  expect(resumeKind(record('a'))).toBe('adopt');
  expect(
    resumeKind(record('a', { phase: 'ended', error: LOST_IN_BACKGROUND })),
  ).toBe('reopen');
  expect(
    resumeKind(
      record('a', {
        phase: 'ended',
        error: {
          code: 'authFailed',
          rsCode: ADAPTER_RS_E.authFailed,
          message: 'no',
        },
      }),
    ),
  ).toBe('error');
});

it('matches a target by Host id, or by address and port', () => {
  expect(sameTarget(vnc, { protocol: 'vnc', hostId: 'h1', addr: 'x' })).toBe(
    true,
  );
  expect(sameTarget(vnc, { protocol: 'rdp', hostId: 'h1', addr: 'x' })).toBe(
    false,
  );
  const typed: AdapterTarget = { protocol: 'vnc', addr: '10.0.0.2' };
  expect(sameTarget(typed, { ...typed, port: 5900 })).toBe(true);
  expect(sameTarget(typed, { ...typed, port: 5901 })).toBe(false);
});

it('lists records oldest first and finds one by target', () => {
  adapterLive.put(record('late', { startedAtUs: 9 }));
  adapterLive.put(
    record('early', {
      startedAtUs: 3,
      params: { target: { protocol: 'rdp', addr: '10.0.0.3' } },
    }),
  );
  const state = adapterLive.getState();
  expect(selectCollapsed(state).map(r => r.sessionId)).toEqual([
    'early',
    'late',
  ]);
  expect(findCollapsed(state, vnc)?.sessionId).toBe('late');
  expect(
    findCollapsed(state, { protocol: 'vnc', addr: '10.0.0.3' }),
  ).toBeUndefined();
});

it('an ended record keeps the reason it ended with', () => {
  const ended = record('a', { phase: 'ended', error: LOST_IN_BACKGROUND });
  const state = { a: ended };
  expect(
    applyAdapterLiveEvent(
      state,
      {
        type: 'adapter.state',
        sessionId: 'a',
        protocol: 'vnc',
        phase: 'ended',
        credError: false,
      },
      5,
    ),
  ).toBe(state);
});

it('leaves the session id out of the route params', () => {
  expect(routeParamsOf({ target: vnc, name: 'Mac mini', sessionId: 'a' })).toEqual(
    { target: vnc, name: 'Mac mini' },
  );
});
