import type { RootStackParamList } from '../../app/navigation/routes';
import type { AdapterRouteParams } from '../adapters/liveStore';
import type { SwitcherItem } from './switcher';

/**
 * the session a window of its own shows. It travels to the native
 * side as an opaque string (the scene's NSUserActivity) and comes back as the
 * window root's initial props, also when iPadOS restores the window.
 */
export type WindowTarget =
  | { kind: 'host'; sessionId: string; hostId: string; name: string }
  | {
      kind: 'vnc' | 'rdp';
      sessionId: string;
      name: string;
      params: AdapterRouteParams;
    };

/** Terminals stay in the main window. */
export function windowTargetOf(item: SwitcherItem): WindowTarget | null {
  switch (item.kind) {
    case 'host':
      return {
        kind: 'host',
        sessionId: item.id,
        hostId: item.hostId,
        name: item.name,
      };
    case 'vnc':
    case 'rdp': {
      const params = { ...item.params };
      delete params.switchSeq;
      return { kind: item.kind, sessionId: item.id, name: item.name, params };
    }
    case 'ssh':
      return null;
  }
}

export function encodeWindowTarget(target: WindowTarget): string {
  return JSON.stringify(target);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function nonEmpty(value: unknown): value is string {
  return typeof value === 'string' && value !== '';
}

export function decodeWindowTarget(text: unknown): WindowTarget | null {
  if (typeof text !== 'string') {
    return null;
  }
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isRecord(value) || !nonEmpty(value.sessionId)) {
    return null;
  }
  const name = typeof value.name === 'string' ? value.name : '';
  if (value.kind === 'host') {
    return nonEmpty(value.hostId)
      ? { kind: 'host', sessionId: value.sessionId, hostId: value.hostId, name }
      : null;
  }
  if (value.kind !== 'vnc' && value.kind !== 'rdp') {
    return null;
  }
  const params = value.params;
  if (
    !isRecord(params) ||
    !isRecord(params.target) ||
    params.target.protocol !== value.kind ||
    !nonEmpty(params.target.addr)
  ) {
    return null;
  }
  return {
    kind: value.kind,
    sessionId: value.sessionId,
    name,
    params: params as AdapterRouteParams,
  };
}

export type WindowRoute =
  | { name: 'Session'; params: RootStackParamList['Session'] }
  | { name: 'AdapterSession'; params: RootStackParamList['AdapterSession'] };

/** The session page the window opens straight away. */
export function windowRoute(target: WindowTarget): WindowRoute {
  if (target.kind === 'host') {
    return {
      name: 'Session',
      params: { hostId: target.hostId, sessionId: target.sessionId },
    };
  }
  return {
    name: 'AdapterSession',
    params: { ...target.params, sessionId: target.sessionId },
  };
}

/**
 * A window iPadOS opened by itself (the multitasking menu, App Exposé's +)
 * while the main window is up: it starts on the computer list (选电脑).
 * The native side writes this payload (NativeLogic WindowActivity).
 */
export type WindowPicker = { kind: 'picker' };

export type WindowPayload = WindowTarget | WindowPicker;

export const PICKER_PAYLOAD = '{"kind":"picker"}';

/** What a window of its own shows: one session, or the computer list. */
export function decodeWindowPayload(text: unknown): WindowPayload | null {
  if (typeof text === 'string') {
    try {
      const value: unknown = JSON.parse(text);
      if (isRecord(value) && value.kind === 'picker') {
        return { kind: 'picker' };
      }
    } catch {
      return null;
    }
  }
  return decodeWindowTarget(text);
}
