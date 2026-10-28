import type { SessionSummary } from '../../ipc/types';
import type { TrayState } from '../../platform/types';
import { zh } from '../../i18n';

export interface Notice {
  title: string;
  body: string;
}

export function trayStateOf(input: {
  connected: boolean;
  sessions: number;
  updateVersion: string | null;
}): TrayState {
  if (!input.connected) {
    return 'offline';
  }
  if (input.sessions > 0) {
    return 'controlled';
  }
  if (input.updateVersion !== null) {
    return 'update';
  }
  return 'idle';
}

function findController(sessions: SessionSummary[]): SessionSummary | null {
  return sessions.find((session) => session.role === 'controller') ?? null;
}

function isSessionIn(sessions: SessionSummary[], sessionId: string): boolean {
  return sessions.some((session) => session.session_id === sessionId);
}

export function diffSessions(prev: SessionSummary[] | null, next: SessionSummary[]): Notice[] {
  if (prev === null) {
    return [];
  }
  if (prev.length === 0) {
    return [];
  }

  const previousController = findController(prev);
  const nextController = findController(next);

  if (
    previousController !== null &&
    nextController !== null &&
    previousController.session_id !== nextController.session_id
  ) {
    const body = isSessionIn(next, previousController.session_id)
      ? zh.bg.handoffBody(previousController.device_name)
      : zh.bg.handoffShortBody;
    return [{ title: zh.bg.handoffTitle(nextController.device_name), body }];
  }

  if (nextController === null) {
    return [];
  }

  const previousIds = new Set(prev.map((session) => session.session_id));
  return next
    .filter((session) => session.role === 'viewer' && !previousIds.has(session.session_id))
    .map((session) => ({
      title: zh.bg.watchTitle(session.device_name),
      body: zh.bg.watchBody(nextController.device_name, session.device_name),
    }));
}
