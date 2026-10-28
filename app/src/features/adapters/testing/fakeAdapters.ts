import type { FakeCore } from '../../../native/rscore/fakeCore';
import type { RsCoreApi } from '../../../native/rscore/RsCoreApi';
import type {
  AdapterScreen,
  AdapterCredentials,
  AdapterOpenOpts,
  AdapterSummary,
  AdapterTarget,
  SavedCredentialInfo,
  SessionId,
} from '../../../native/rscore/types';

export type FakeAdapterCall = { method: string; args: unknown[] };

export type FakeAdapters = NonNullable<RsCoreApi['adapters']> & {
  readonly calls: FakeAdapterCall[];
  holdNextOpen(): void;
  resolveOpen(): SessionId;
  rejectOpen(err: Error): void;
  failNext(method: string, err: Error): void;
  setSaved(target: AdapterTarget, info: SavedCredentialInfo): void;
  setCanSave(v: boolean): void;
  setAvailable(v: boolean): void;
  emitScreens(id: SessionId, screens: AdapterScreen[]): void;
  /** Forgets an open session without a close call, like a dead socket. */
  dropSession(id: SessionId): void;
};

const FIRST_ID = 2n ** 62n + 1n;

function clone<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map(item => clone(item)) as T;
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, clone(item)]),
    ) as T;
  }
  return value;
}

export function installFakeAdapters(
  core: FakeCore,
  opts: { available?: boolean; canSave?: boolean } = {},
): FakeAdapters {
  const calls: FakeAdapterCall[] = [];
  const failures = new Map<string, Error>();
  const saved = new Map<string, SavedCredentialInfo>();
  const sessions = new Map<SessionId, AdapterTarget>();
  let available = opts.available ?? true;
  let canSave = opts.canSave ?? true;
  let nextId = FIRST_ID;
  let holdOpen = false;
  let pendingOpen:
    | {
        id: SessionId;
        resolve: (id: SessionId) => void;
        reject: (error: Error) => void;
      }
    | undefined;

  const record = (method: string, ...args: unknown[]): void => {
    calls.push({ method, args });
    const error = failures.get(method);
    if (error !== undefined) {
      failures.delete(method);
      throw error;
    }
  };

  const open = (
    target: AdapterTarget,
    openOpts: AdapterOpenOpts,
  ): Promise<SessionId> => {
    record('open', target, openOpts);
    const id = String(nextId++);
    sessions.set(id, target);
    if (holdOpen) {
      holdOpen = false;
      return new Promise<SessionId>((resolve, reject) => {
        pendingOpen = { id, resolve, reject };
      });
    }
    return Promise.resolve(id);
  };

  const adapters: FakeAdapters = {
    calls,
    available: () => {
      record('available');
      return available;
    },
    open,
    close: id => {
      record('close', id);
      sessions.delete(id);
    },
    list: (): AdapterSummary[] => {
      record('list');
      return [...sessions].map(([sessionId, target]) => ({
        sessionId,
        target: clone(target),
        phase: 'streaming',
      }));
    },
    submitCredentials: (id: SessionId, credentials: AdapterCredentials) => {
      record('submitCredentials', id, credentials);
    },
    cancelCredentials: id => {
      record('cancelCredentials', id);
    },
    saved: target => {
      record('saved', target);
      return clone(saved.get(JSON.stringify(target)) ?? { saved: false });
    },
    forget: target => {
      record('forget', target);
      saved.delete(JSON.stringify(target));
    },
    canSave: () => {
      record('canSave');
      return canSave;
    },
    typePassword: id => {
      record('typePassword', id);
    },
    sendCtrlAltDel: id => {
      record('sendCtrlAltDel', id);
    },
    trustCert: (id, accept) => {
      record('trustCert', id, accept);
    },
    resize: (id, viewWidth, viewHeight, scale) => {
      record('resize', id, viewWidth, viewHeight, scale);
    },
    selectScreen: (id, index) => {
      record('selectScreen', id, index);
    },
    setMonitors: (id, count) => {
      record('setMonitors', id, count);
    },
    holdNextOpen: () => {
      holdOpen = true;
    },
    resolveOpen: () => {
      const pending = pendingOpen;
      if (pending === undefined) {
        return String(nextId++);
      }
      pendingOpen = undefined;
      pending.resolve(pending.id);
      return pending.id;
    },
    rejectOpen: (error: Error) => {
      const pending = pendingOpen;
      if (pending === undefined) {
        return;
      }
      pendingOpen = undefined;
      pending.reject(error);
    },
    failNext: (method, error) => {
      failures.set(method, error);
    },
    setSaved: (target, info) => {
      saved.set(JSON.stringify(target), clone(info));
    },
    setCanSave: value => {
      canSave = value;
    },
    setAvailable: value => {
      available = value;
    },
    dropSession: id => {
      sessions.delete(id);
    },
    emitScreens: (id, screens) => {
      core.emit({
        type: 'adapter.screens',
        sessionId: id,
        screens: clone(screens),
      });
    },
  };

  Object.assign(core, { adapters });
  return adapters;
}
