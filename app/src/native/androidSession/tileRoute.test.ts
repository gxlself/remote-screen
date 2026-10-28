import type { SessionSummary } from '../rscore/types';
import { routeForNotification, routeForTile } from './tileRoute';

const first: SessionSummary = {
  id: 'session-1',
  hostId: 'host-1',
  hostName: 'Studio Mac mini',
  phase: 'streaming',
  role: null,
  path: null,
  transport: null,
  startedAt: 1_800_000_000_000_001,
  displayId: null,
};
const second: SessionSummary = {
  ...first,
  id: 'session-2',
  hostId: 'host-2',
  startedAt: 1_800_000_000_000_999,
};

it('resumes a live session for an empty tile hostId', () => {
  expect(routeForTile('', [first])).toEqual({
    hostId: first.hostId,
    sessionId: first.id,
  });
});

it('has no resume target without live sessions', () => {
  expect(routeForTile('', [])).toBeNull();
});

it('resumes the live session on the requested host', () => {
  expect(routeForTile(first.hostId, [first, second])).toEqual({
    hostId: first.hostId,
    sessionId: first.id,
  });
});

it('opens a new session when the requested host has no live session', () => {
  expect(routeForTile('host-3', [first, second])).toEqual({
    hostId: 'host-3',
  });
});

it('chooses the most recently started session even when the input is unsorted', () => {
  const live = [second, first];
  expect(routeForTile('', live)).toEqual({
    hostId: second.hostId,
    sessionId: second.id,
  });
  expect(live).toEqual([second, first]);
});

it('chooses the most recent session for a host with multiple live sessions', () => {
  const newer = { ...second, hostId: first.hostId };
  expect(routeForTile(first.hostId, [newer, first])).toEqual({
    hostId: first.hostId,
    sessionId: newer.id,
  });
});

it('opens the live session named by a notification', () => {
  expect(routeForNotification(first.id, [second, first])).toEqual({
    hostId: first.hostId,
    sessionId: first.id,
  });
});

it('ignores a notification for a missing session', () => {
  expect(routeForNotification('missing', [first, second])).toBeNull();
});

it('does not resume an ended session', () => {
  const ended = { ...first, phase: 'ended' as const };
  expect(routeForTile('', [ended])).toBeNull();
  expect(routeForTile(first.hostId, [ended])).toEqual({ hostId: first.hostId });
  expect(routeForNotification(first.id, [ended])).toBeNull();
});
