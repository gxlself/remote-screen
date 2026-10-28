import type { ApprovalIntent, SessionSummary } from './types';

export type ApprovalKind = 'connection' | 'pairing' | 'takeover';

export interface ApprovalRequest {
  request_id: string;
  kind: ApprovalKind;
  intent?: ApprovalIntent;
  device_name: string;
  device_id: string;
  platform: string;
  whois_login: string | null;
  path: string | null;
  reasons: string[];
  timeout_s: number;
  permission_choice: boolean;
  device_fp: string | null;
}

export type ApprovalOutcome = 'allowed' | 'denied' | 'timeout' | 'cancelled';

export interface ApprovalClosedEvent {
  request_id: string;
  kind: ApprovalKind;
  outcome: ApprovalOutcome;
}

export const PAIR_STATUS = {
  UNSPECIFIED: 0,
  OK: 1,
  WAITING_LOCAL: 2,
  DENIED: 3,
  BAD_CODE: 4,
  EXPIRED: 5,
  LOCKED: 6,
  DISABLED_ON_PATH: 7,
} as const;

export interface PairingResultEvent {
  status: number;
  host_id: string;
  host_name: string;
  attempts_left: number;
  device_fp: string;
  method?: 'code' | 'file' | 'password' | 'approve';
  device_name?: string;
  account?: string;
}

export interface SessionsChangedEvent {
  sessions?: SessionSummary[];
}

export interface UpdateStateEvent {
  state?: string;
}

export type PermissionsChangedEvent = Record<string, unknown>;

export interface HandoffEvent {
  session_id?: string;
  [key: string]: unknown;
}

export interface IpcEvents {
  'event.approvalRequested': ApprovalRequest;
  'event.approvalClosed': ApprovalClosedEvent;
  'event.sessionsChanged': SessionsChangedEvent;
  'event.pairingResult': PairingResultEvent;
  'event.updateState': UpdateStateEvent;
  'event.permissionsChanged': PermissionsChangedEvent;
  'event.handoff': HandoffEvent;
}

export type EventName = keyof IpcEvents;

export const EVENT_NAMES = [
  'event.approvalRequested',
  'event.approvalClosed',
  'event.sessionsChanged',
  'event.pairingResult',
  'event.updateState',
  'event.permissionsChanged',
  'event.handoff',
] as const satisfies readonly EventName[];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isApprovalKind(value: unknown): value is ApprovalKind {
  return value === 'connection' || value === 'pairing' || value === 'takeover';
}

function isApprovalOutcome(value: unknown): value is ApprovalOutcome {
  return value === 'allowed' || value === 'denied' || value === 'timeout' || value === 'cancelled';
}

function stringOr(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function nullableString(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

function numberOr(value: unknown, fallback: number): number {
  return typeof value === 'number' ? value : fallback;
}

function pairingMethod(value: unknown): PairingResultEvent['method'] | undefined {
  return value === 'code' || value === 'file' || value === 'password' || value === 'approve'
    ? value
    : undefined;
}

function parseApprovalRequest(value: unknown): ApprovalRequest | null {
  if (!isRecord(value) || typeof value.request_id !== 'string' || !isApprovalKind(value.kind)) {
    return null;
  }

  const reasons =
    Array.isArray(value.reasons) &&
    value.reasons.every((reason): reason is string => typeof reason === 'string')
      ? value.reasons
      : [];

  return {
    request_id: value.request_id,
    kind: value.kind,
    ...(value.intent === 'control' || value.intent === 'view' || value.intent === 'files'
      ? { intent: value.intent }
      : {}),
    device_name: stringOr(value.device_name, ''),
    device_id: stringOr(value.device_id, ''),
    platform: stringOr(value.platform, 'unknown'),
    whois_login: nullableString(value.whois_login),
    path: nullableString(value.path),
    reasons,
    timeout_s: numberOr(value.timeout_s, 30),
    permission_choice: value.permission_choice === true,
    device_fp: nullableString(value.device_fp),
  };
}

function parseApprovalClosed(value: unknown): ApprovalClosedEvent | null {
  if (
    !isRecord(value) ||
    typeof value.request_id !== 'string' ||
    !isApprovalKind(value.kind) ||
    !isApprovalOutcome(value.outcome)
  ) {
    return null;
  }

  return {
    request_id: value.request_id,
    kind: value.kind,
    outcome: value.outcome,
  };
}

function parsePairingResult(value: unknown): PairingResultEvent | null {
  if (!isRecord(value) || typeof value.status !== 'number') {
    return null;
  }

  const method = pairingMethod(value.method);
  return {
    status: value.status,
    host_id: stringOr(value.host_id, ''),
    host_name: stringOr(value.host_name, ''),
    attempts_left: numberOr(value.attempts_left, 0),
    device_fp: stringOr(value.device_fp, ''),
    ...(method === undefined ? {} : { method }),
    ...(typeof value.device_name === 'string' ? { device_name: value.device_name } : {}),
    ...(typeof value.account === 'string' ? { account: value.account } : {}),
  };
}

function parseSessionsChanged(value: unknown): SessionsChangedEvent {
  if (!isRecord(value) || !Array.isArray(value.sessions)) {
    return {};
  }

  return { sessions: value.sessions as SessionSummary[] };
}

function parseUpdateState(value: unknown): UpdateStateEvent {
  if (!isRecord(value) || typeof value.state !== 'string') {
    return {};
  }

  return { state: value.state };
}

function parseObjectEvent(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) {
    return {};
  }

  return { ...value };
}

function parseKnownEvent(name: EventName, params: unknown): IpcEvents[EventName] | null {
  switch (name) {
    case 'event.approvalRequested':
      return parseApprovalRequest(params);
    case 'event.approvalClosed':
      return parseApprovalClosed(params);
    case 'event.sessionsChanged':
      return parseSessionsChanged(params);
    case 'event.pairingResult':
      return parsePairingResult(params);
    case 'event.updateState':
      return parseUpdateState(params);
    case 'event.permissionsChanged':
      return parseObjectEvent(params);
    case 'event.handoff':
      return parseObjectEvent(params) as HandoffEvent;
  }
}

export function parseEvent<E extends EventName>(name: E, params: unknown): IpcEvents[E] | null {
  return parseKnownEvent(name, params) as IpcEvents[E] | null;
}
