import type { RemoteFallbackTarget } from '../fallback/useRemoteFallback';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type {
  AdapterCredField,
  AdapterCredentials,
  AdapterErrorCode,
  AdapterErrorInfo,
  AdapterPhase,
  AdapterProtocol,
  AdapterTarget,
  Address,
  Computer,
  SessionId,
} from '../../native/rscore/types';
import { ADAPTER_RS_E } from '../../native/rscore/types';

export const BUILTIN_PROTOCOLS = ['vnc', 'rdp'] as const;

export type AdapterAddrKind = Address['kind'] | 'manual';

const DEFAULT_PORT: Record<AdapterProtocol, number> = {
  vnc: 5900,
  rdp: 3389,
};

export function adapterProtocolFor(os: Computer['os']): AdapterProtocol {
  return os === 'windows' ? 'rdp' : 'vnc';
}

export function targetForComputer(
  c: Pick<Computer, 'hostId' | 'os' | 'addresses'>,
): { target: AdapterTarget; addrKind: Address['kind'] } | null {
  for (const kind of ['tailnet', 'intranet', 'lan'] as const) {
    const address = c.addresses.find(candidate => candidate.kind === kind);
    if (address !== undefined) {
      return {
        target: {
          protocol: adapterProtocolFor(c.os),
          hostId: c.hostId,
          addr: address.addr,
        },
        addrKind: kind,
      };
    }
  }
  return null;
}

export function targetForAddress(
  protocol: AdapterProtocol,
  addr: string,
  port?: number,
): AdapterTarget | null {
  let normalized = addr.trim();
  if (normalized.startsWith('[') && normalized.endsWith(']')) {
    normalized = normalized.slice(1, -1);
  }
  if (normalized.length === 0) {
    return null;
  }
  if (
    port !== undefined &&
    (!Number.isInteger(port) || port < 1 || port > 65535)
  ) {
    return null;
  }

  const target: AdapterTarget = { protocol, addr: normalized };
  if (port !== undefined && port !== DEFAULT_PORT[protocol]) {
    target.port = port;
  }
  return target;
}

export const CRED_LIMITS = { user: 256, password: 1024, domain: 256 } as const;
export type CredFieldError = 'empty' | 'length' | 'control';
export type CredInput = { user: string; password: string; domain: string };

function hasControlCharacter(text: string): boolean {
  for (const character of text) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) {
      return true;
    }
  }
  return false;
}

function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

export function validateCredentials(
  fields: readonly AdapterCredField[],
  v: CredInput,
): Partial<Record<AdapterCredField, CredFieldError>> {
  const errors: Partial<Record<AdapterCredField, CredFieldError>> = {};
  const fieldSet = new Set(fields);
  const values: Record<AdapterCredField, string> = {
    user: fieldSet.has('user') ? v.user.trim() : '',
    password: v.password,
    domain: fieldSet.has('domain') ? v.domain.trim() : '',
  };

  const required: Record<AdapterCredField, boolean> = {
    user: fieldSet.has('user'),
    password: true,
    domain: false,
  };

  for (const field of ['user', 'password', 'domain'] as const) {
    if (field !== 'password' && !fieldSet.has(field)) {
      continue;
    }
    const value = values[field];
    if (required[field] && value.length === 0) {
      errors[field] = 'empty';
    } else if (utf8Length(value) > CRED_LIMITS[field]) {
      errors[field] = 'length';
    } else if (hasControlCharacter(value)) {
      errors[field] = 'control';
    }
  }
  return errors;
}

export function toCredentials(
  fields: readonly AdapterCredField[],
  v: CredInput,
  save: boolean,
): AdapterCredentials {
  const fieldSet = new Set(fields);
  const user = fieldSet.has('user') ? v.user.trim() : undefined;
  const domain = fieldSet.has('domain') ? v.domain.trim() : undefined;
  return {
    user: user === '' ? undefined : user,
    password: fieldSet.has('password')
      ? v.password
      : (undefined as unknown as string),
    domain: domain === '' ? undefined : domain,
    save,
  };
}

export function adaptersAvailable(core: RsCoreApi): boolean {
  try {
    return core.adapters?.available() === true;
  } catch {
    return false;
  }
}

export function isEncryptedPath(
  protocol: AdapterProtocol,
  addrKind: AdapterAddrKind | undefined,
): boolean {
  return protocol === 'rdp' || (protocol === 'vnc' && addrKind === 'tailnet');
}

export function isLatin1(text: string): boolean {
  for (const character of text) {
    if ((character.codePointAt(0) ?? 0) > 0xff) {
      return false;
    }
  }
  return true;
}

export function fallbackTargetFor(target: AdapterTarget): RemoteFallbackTarget {
  const fallback: RemoteFallbackTarget = {
    os: target.protocol === 'vnc' ? 'macos' : 'windows',
    addr: target.addr,
  };
  if (target.port !== undefined) {
    fallback.port = target.port;
  }
  return fallback;
}

export type AdapterUiPhase = 'opening' | AdapterPhase;
export type AdapterEvent = Extract<
  import('../../native/rscore/types').CoreEvent,
  { type: 'adapter.state' | 'adapter.certPrompt' }
>;
export type AdapterViewState = {
  sessionId: SessionId | null;
  phase: AdapterUiPhase;
  credFields: AdapterCredField[];
  credError: boolean;
  width?: number;
  height?: number;
  serverName?: string;
  error: AdapterErrorInfo | null;
  cert: { fp: string; oldFp?: string } | null;
  pending: AdapterEvent[];
};

export type AdapterAction =
  | { type: 'reset' }
  | { type: 'opened'; id: SessionId }
  | { type: 'openFailed'; message: string }
  | { type: 'event'; event: AdapterEvent }
  | { type: 'certAnswered' }
  /** The page ended the session itself (lost in the background, gave up). */
  | { type: 'lost'; error: AdapterErrorInfo };

export const initialAdapterState: AdapterViewState = {
  sessionId: null,
  phase: 'opening',
  credFields: [],
  credError: false,
  error: null,
  cert: null,
  pending: [],
};

function stateForEvent(
  state: AdapterViewState,
  event: Extract<AdapterEvent, { type: 'adapter.state' }>,
): AdapterViewState {
  const next: AdapterViewState = {
    ...state,
    phase: event.phase,
    credFields: event.credFields ?? [],
    credError: event.credError,
    width: event.width,
    height: event.height,
    serverName: event.serverName,
    error: event.error ?? null,
  };
  if (event.phase === 'streaming' || event.phase === 'ended') {
    next.cert = null;
  }
  return next;
}

function applyEvent(
  state: AdapterViewState,
  event: AdapterEvent,
): AdapterViewState {
  if (event.type === 'adapter.state') {
    return stateForEvent(state, event);
  }
  return {
    ...state,
    cert: {
      fp: event.fp,
      ...(event.oldFp === undefined ? {} : { oldFp: event.oldFp }),
    },
  };
}

function freshConnectionState(id: SessionId): AdapterViewState {
  return {
    ...initialAdapterState,
    sessionId: id,
    phase: 'connecting',
  };
}

export function adapterReducer(
  state: AdapterViewState,
  action: AdapterAction,
): AdapterViewState {
  switch (action.type) {
    case 'reset':
      return initialAdapterState;
    case 'opened': {
      let next = freshConnectionState(action.id);
      for (const event of state.pending) {
        if (event.sessionId === action.id) {
          next = applyEvent(next, event);
        }
      }
      return next;
    }
    case 'openFailed':
      return {
        ...state,
        phase: 'ended',
        error: {
          code: 'io',
          rsCode: ADAPTER_RS_E.broken,
          message: action.message,
        },
      };
    case 'certAnswered':
      return { ...state, cert: null };
    case 'lost':
      return { ...state, phase: 'ended', error: action.error, cert: null };
    case 'event':
      if (state.sessionId === null) {
        const pending = [...state.pending, action.event];
        return {
          ...state,
          pending: pending.length > 32 ? pending.slice(-32) : pending,
        };
      }
      if (action.event.sessionId !== state.sessionId) {
        return state;
      }
      return applyEvent(state, action.event);
  }
}

/** Pauses before each automatic reconnect; its length is the attempt count. */
export const RECONNECT_DELAYS_MS = [1000, 2000, 4000] as const;

/**
 * Errors worth reconnecting by themselves: the link dropped or stopped
 * answering. Refused (sharing turned off), authentication, certificates and
 * the computer closing the session all need the user.
 */
export function isNetworkDrop(code: AdapterErrorCode): boolean {
  return code === 'timeout' || code === 'io';
}

export const LOST_IN_BACKGROUND: AdapterErrorInfo = {
  code: 'io',
  rsCode: ADAPTER_RS_E.broken,
  message: 'session gone after returning to the foreground',
};

export type ToolbarDock = 'bottom' | 'left' | 'right';

export const TOOLBAR_DOCKS: readonly ToolbarDock[] = [
  'bottom',
  'left',
  'right',
];

/** Where a toolbar dropped at x docks: the outer thirds pick a side. */
export function dockForPoint(x: number, width: number): ToolbarDock {
  if (x < width / 3) {
    return 'left';
  }
  if (x > (width * 2) / 3) {
    return 'right';
  }
  return 'bottom';
}

/** Splits a colon-separated fingerprint into lines of `perLine` bytes. */
export function fingerprintLines(fp: string, perLine = 8): string[] {
  const bytes = fp.split(':');
  const lines: string[] = [];
  for (let index = 0; index < bytes.length; index += perLine) {
    lines.push(bytes.slice(index, index + perLine).join(':'));
  }
  return lines;
}
