import type {
  AdapterRouteParams,
  CollapsedAdapter,
} from '../adapters/liveStore';
import type { SshRecord } from '../sessions/sshStore';
import type { SessionShortcutEvent } from '../../native/keyboard/KeyCapture';
import type { AdapterPhase, SessionPhase } from '../../native/rscore/types';
import type { SessionRecord } from './store';

/** Above this many open sessions the switcher says it costs battery. */
export const MANY_SESSIONS = 4;

export type SwitcherStatus =
  | 'connected'
  | 'connecting'
  | 'reconnecting'
  | 'disconnected';

type ItemBase = {
  /** The session id (Host / adapter) or the SSH id. */
  id: string;
  name: string;
  status: SwitcherStatus;
  /** Orders the list; numbers stay put while sessions come and go. */
  startedAtUs: number;
};

export type SwitcherItem =
  | (ItemBase & { kind: 'host'; hostId: string })
  | (ItemBase & { kind: 'vnc' | 'rdp'; params: AdapterRouteParams })
  | (ItemBase & { kind: 'ssh'; serverId: string });

export type SwitcherList = {
  /** Full-screen sessions, numbered 1… for ⌃⌥⌘1…9 and ⌃⌥⌘←/→. */
  screens: SwitcherItem[];
  /** Open terminals: listed, but not reached by the shortcuts. */
  terminals: SwitcherItem[];
};

export function hostSwitcherStatus(phase: SessionPhase): SwitcherStatus {
  switch (phase) {
    case 'streaming':
      return 'connected';
    case 'reconnecting':
      return 'reconnecting';
    case 'ended':
      return 'disconnected';
    default:
      return 'connecting';
  }
}

export function adapterSwitcherStatus(
  /** `opening`: the page is asking for a session id. */
  phase: AdapterPhase | 'opening',
  reconnecting = false,
): SwitcherStatus {
  if (reconnecting) {
    return 'reconnecting';
  }
  switch (phase) {
    case 'streaming':
      return 'connected';
    case 'ended':
      return 'disconnected';
    default:
      return 'connecting';
  }
}

export function hostSwitcherItem(
  record: SessionRecord,
  name: string,
): SwitcherItem {
  return {
    kind: 'host',
    id: record.id,
    hostId: record.hostId,
    name,
    status: hostSwitcherStatus(record.phase),
    startedAtUs: record.startedAt,
  };
}

export function adapterSwitcherItem(record: CollapsedAdapter): SwitcherItem {
  return {
    kind: record.params.target.protocol,
    id: record.sessionId,
    params: record.params,
    name: record.name,
    status: adapterSwitcherStatus(record.phase),
    startedAtUs: record.startedAtUs,
  };
}

export function sshSwitcherItem(record: SshRecord, name: string): SwitcherItem {
  return {
    kind: 'ssh',
    id: record.sshId,
    serverId: record.serverId,
    name,
    status: record.state === 'open' ? 'connected' : 'connecting',
    startedAtUs: record.openedAtUs ?? record.updatedAtUs,
  };
}

function byStart(left: SwitcherItem, right: SwitcherItem): number {
  if (left.startedAtUs !== right.startedAtUs) {
    return left.startedAtUs - right.startedAtUs;
  }
  return left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
}

/**
 * Every open session once, oldest first. `current` is the page's own session
 * (an adapter page holds it outside the folded-away store) and replaces any
 * stale copy with the same id.
 */
export function buildSwitcherList(
  items: readonly SwitcherItem[],
  current: SwitcherItem | null = null,
): SwitcherList {
  const byId = new Map<string, SwitcherItem>();
  for (const item of items) {
    byId.set(item.id, item);
  }
  if (current !== null) {
    byId.set(current.id, current);
  }
  const all = [...byId.values()].sort(byStart);
  return {
    screens: all.filter(item => item.kind !== 'ssh'),
    terminals: all.filter(item => item.kind === 'ssh'),
  };
}

export function sessionCount(list: SwitcherList): number {
  return list.screens.length + list.terminals.length;
}

export function showsManyNote(list: SwitcherList): boolean {
  return sessionCount(list) > MANY_SESSIONS;
}

/**
 * Where a ⌃⌥⌘ shortcut goes from `currentId`: next / previous wrap around,
 * a number past the list or the session already shown does nothing (null).
 * `switcher` opens the overlay and `toolbar` shows or hides the toolbar;
 * neither has a target.
 */
export function shortcutTarget(
  screens: readonly SwitcherItem[],
  currentId: string | null,
  event: SessionShortcutEvent,
): SwitcherItem | null {
  if (screens.length === 0) {
    return null;
  }
  const at = screens.findIndex(item => item.id === currentId);
  let target: SwitcherItem | undefined;
  switch (event.action) {
    case 'next':
      target = screens[at < 0 ? 0 : (at + 1) % screens.length];
      break;
    case 'previous':
      target =
        screens[
          at < 0 ? screens.length - 1 : (at - 1 + screens.length) % screens.length
        ];
      break;
    case 'index':
      target = screens[event.index - 1];
      break;
    case 'switcher':
    case 'toolbar':
      return null;
  }
  return target === undefined || target.id === currentId ? null : target;
}
