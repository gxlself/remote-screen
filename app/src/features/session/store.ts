import { createContext, createElement, useContext, useMemo } from 'react';
import type { ReactNode } from 'react';
import { create, useStore } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import type { StoreApi, UseBoundStore } from 'zustand';
import type {
  CoreEvent,
  CoreEventOf,
  DialAttempt,
  GoodbyeReason,
  Micros,
  Participant,
  SessionDisplay,
  SessionId,
  SessionSummary,
} from '../../native/rscore/types';

export type SessionRecord = SessionSummary & {
  controller: Participant | null;
  viewers: Participant[];
  inputPausedLocal: boolean;
  stats: CoreEventOf<'session.stats'> | null;
  displays: SessionDisplay[];
  dial: DialAttempt[];
  approvalTimeoutS: number | null;
  derpRegion: string | null;
  reconnectReason: 'pathLost' | 'hostUpdating' | 'resume' | null;
  idleWarning: {
    secondsLeft: number;
    lockOnEnd: boolean;
    atUs: Micros;
  } | null;
  controlRequest: {
    requester: Participant;
    timeoutS: number;
    atUs: Micros;
  } | null;
  lastError: {
    code?: number;
    reason?: GoodbyeReason;
    detail?: Record<string, string>;
    message?: string;
  } | null;
  clipboard: {
    direction: 'fromHost' | 'toHost';
    text: string;
    at: Micros;
  }[];
};

export type SessionsState = Record<SessionId, SessionRecord>;

export type SessionsActions = {
  dismiss(id: SessionId): void;
  seed(list: SessionSummary[]): void;
  clearIdleWarning(id: SessionId): void;
  clearControlRequest(id: SessionId): void;
};

export type SessionsStore = UseBoundStore<StoreApi<SessionsState>> &
  SessionsActions;

function newRecord(summary: SessionSummary, startedAt: Micros): SessionRecord {
  return {
    ...summary,
    startedAt,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays: [],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
  };
}

function update(
  state: SessionsState,
  sessionId: SessionId,
  apply: (record: SessionRecord) => SessionRecord,
): SessionsState {
  const record = state[sessionId];
  if (record === undefined) {
    return state;
  }
  return { ...state, [sessionId]: apply(record) };
}

export function applySessionEvent(
  state: SessionsState,
  event: CoreEvent,
  nowUs: Micros,
): SessionsState {
  switch (event.type) {
    case 'session.state': {
      const current = state[event.sessionId];
      if (current === undefined) {
        const record = newRecord(
          {
            id: event.sessionId,
            hostId: event.hostId,
            hostName: event.hostId,
            phase: event.phase,
            role: null,
            path: event.path ?? null,
            transport: event.transport ?? null,
            startedAt: nowUs,
            displayId: null,
          },
          nowUs,
        );
        record.derpRegion = event.derpRegion ?? null;
        record.dial = event.dial ?? [];
        record.approvalTimeoutS = event.approvalTimeoutS ?? null;
        record.reconnectReason =
          event.phase === 'streaming' ? null : event.reconnectReason ?? null;
        return { ...state, [event.sessionId]: record };
      }
      const streaming = event.phase === 'streaming';
      return {
        ...state,
        [event.sessionId]: {
          ...current,
          phase: event.phase,
          path: event.path ?? null,
          transport: event.transport ?? null,
          derpRegion: event.derpRegion ?? null,
          dial: event.dial ?? [],
          approvalTimeoutS: event.approvalTimeoutS ?? null,
          reconnectReason: streaming ? null : event.reconnectReason ?? null,
          idleWarning: streaming ? null : current.idleWarning,
        },
      };
    }
    case 'session.role':
      return update(state, event.sessionId, record => ({
        ...record,
        role: event.role,
        controller: event.controller,
        viewers: [...event.viewers],
        inputPausedLocal: event.inputPausedLocal,
        controlRequest:
          event.role === 'controller' ? null : record.controlRequest,
      }));
    case 'session.stats':
      return update(state, event.sessionId, record => ({
        ...record,
        stats: event,
        path: event.path,
        transport: event.transport,
        derpRegion: event.derpRegion ?? null,
      }));
    case 'session.displays':
      return update(state, event.sessionId, record => ({
        ...record,
        displays: [...event.displays],
        displayId: event.currentId,
      }));
    case 'session.clipboard':
      return update(state, event.sessionId, record => ({
        ...record,
        clipboard: [
          {
            direction: event.direction,
            text: event.text,
            at: event.at,
          },
          ...record.clipboard,
        ].slice(0, 5),
      }));
    case 'session.idleWarning':
      return update(state, event.sessionId, record => ({
        ...record,
        idleWarning: {
          secondsLeft: event.secondsLeft,
          lockOnEnd: event.lockOnEnd,
          atUs: nowUs,
        },
      }));
    case 'session.controlRequest':
      return update(state, event.sessionId, record => ({
        ...record,
        controlRequest: {
          requester: event.requester,
          timeoutS: event.timeoutS,
          atUs: nowUs,
        },
      }));
    case 'session.error': {
      const lastError: NonNullable<SessionRecord['lastError']> = {};
      if (event.code !== undefined) {
        lastError.code = event.code;
      }
      if (event.reason !== undefined) {
        lastError.reason = event.reason;
      }
      if (event.detail !== undefined) {
        lastError.detail = event.detail;
      }
      if (event.message !== undefined) {
        lastError.message = event.message;
      }
      return update(state, event.sessionId, record => {
        // Rust 用 session.state 发会话结束。推流中的会话收到不带 reason 的 session.error，
        // 是 Host 对某条请求的错误回复（比如 Ctrl+Alt+Del 被策略拒绝），会话还在。
        const keepPhase =
          event.code === 141 ||
          (event.reason === undefined && record.phase === 'streaming');
        return {
          ...record,
          phase: keepPhase ? record.phase : 'ended',
          lastError,
        };
      });
    }
    default:
      return state;
  }
}

export function selectLive(state: SessionsState): SessionRecord[] {
  return Object.values(state)
    .filter(record => record.phase !== 'ended')
    .sort((left, right) => left.startedAt - right.startedAt);
}

export function createSessionsStore(): SessionsStore {
  const store = create<SessionsState>(() => ({}));

  const actions: SessionsActions = {
    dismiss(id) {
      store.setState(state => {
        if (state[id] === undefined) {
          return state;
        }
        const next = { ...state };
        delete next[id];
        return next;
      }, true);
    },
    seed(list) {
      store.setState(state => {
        const next = { ...state };
        for (const summary of list) {
          const current = next[summary.id];
          next[summary.id] =
            current === undefined
              ? newRecord(summary, summary.startedAt)
              : { ...current, ...summary };
        }
        return next;
      });
    },
    clearIdleWarning(id) {
      store.setState(state => {
        const record = state[id];
        if (record === undefined || record.idleWarning === null) {
          return state;
        }
        return { ...state, [id]: { ...record, idleWarning: null } };
      });
    },
    clearControlRequest(id) {
      store.setState(state => {
        const record = state[id];
        if (record === undefined || record.controlRequest === null) {
          return state;
        }
        return { ...state, [id]: { ...record, controlRequest: null } };
      });
    },
  };

  return Object.assign(store, actions) as SessionsStore;
}

const SessionsContext = createContext<SessionsStore | null>(null);

export function SessionsStoreProvider(props: {
  store: SessionsStore;
  children: ReactNode;
}) {
  return createElement(
    SessionsContext.Provider,
    { value: props.store },
    props.children,
  );
}

function useSessionsStore(): SessionsStore {
  const store = useContext(SessionsContext);
  if (store === null) {
    throw new Error('useSessions 必须在 EventsProvider 里使用');
  }
  return store;
}

export function useSessions<T>(selector: (state: SessionsState) => T): T {
  return useStore(useSessionsStore(), useShallow(selector));
}

export function useSessionsActions(): SessionsActions {
  const store = useSessionsStore();
  return useMemo(
    () => ({
      dismiss: store.dismiss,
      seed: store.seed,
      clearIdleWarning: store.clearIdleWarning,
      clearControlRequest: store.clearControlRequest,
    }),
    [store],
  );
}
