import type { SessionSummary } from '../rscore/types';

export type SessionTarget = { hostId: string; sessionId?: string };

export function routeForTile(
  hostId: string,
  live: SessionSummary[],
): SessionTarget | null {
  let latest: SessionSummary | undefined;
  for (const session of live) {
    if (
      session.phase !== 'ended' &&
      (hostId === '' || session.hostId === hostId) &&
      (latest === undefined || session.startedAt > latest.startedAt)
    ) {
      latest = session;
    }
  }
  if (latest !== undefined) {
    return { hostId: latest.hostId, sessionId: latest.id };
  }
  return hostId === '' ? null : { hostId };
}

export function routeForNotification(
  sessionId: string,
  live: SessionSummary[],
): SessionTarget | null {
  const session = live.find(
    item => item.id === sessionId && item.phase !== 'ended',
  );
  return session === undefined
    ? null
    : { hostId: session.hostId, sessionId: session.id };
}
