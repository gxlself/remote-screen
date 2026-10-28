import { create, useStore } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import type { RootStackParamList } from '../../app/navigation/routes';
import type {
  AdapterErrorInfo,
  AdapterPhase,
  AdapterScreen,
  AdapterTarget,
  CoreEvent,
  SessionId,
} from '../../native/rscore/types';
import type { AdapterHistoryTracker } from '../sessions/useAdapterHistory';
import { isNetworkDrop } from './logic';

export type AdapterRouteParams = Omit<
  RootStackParamList['AdapterSession'],
  'sessionId'
>;

/**
 * A Screen Sharing / Remote Desktop session folded away from its page (V33).
 * The page hands it over when it unmounts and takes it back on return.
 */
export type CollapsedAdapter = {
  sessionId: SessionId;
  params: AdapterRouteParams;
  name: string;
  /** The last known phase; `ended` once it dropped or was closed while away. */
  phase: AdapterPhase;
  error: AdapterErrorInfo | null;
  width?: number;
  height?: number;
  serverName?: string;
  screens: AdapterScreen[];
  screenIndex: number;
  mode: 'trackpad' | 'touch';
  /** When the first picture came; the bar and the session tab count from it. */
  startedAtUs: number;
  collapsedAtUs: number;
  history: AdapterHistoryTracker;
};

export type AdapterLiveState = Record<SessionId, CollapsedAdapter>;

/** What the page does with a record it is asked to return to. */
export type ResumeKind =
  /** Still connected: show it again without reconnecting. */
  | 'adopt'
  /** Dropped or folded away too long: connect again by itself. */
  | 'reopen'
  /** Ended for a reason the user has to see: the error page. */
  | 'error';

export function resumeKind(record: CollapsedAdapter): ResumeKind {
  if (record.phase !== 'ended') {
    return 'adopt';
  }
  return record.error === null || isNetworkDrop(record.error.code)
    ? 'reopen'
    : 'error';
}

const DEFAULT_PORT = { vnc: 5900, rdp: 3389 } as const;

/** Whether two targets reach the same computer the same way. */
export function sameTarget(a: AdapterTarget, b: AdapterTarget): boolean {
  if (a.protocol !== b.protocol) {
    return false;
  }
  if (a.hostId !== undefined && a.hostId === b.hostId) {
    return true;
  }
  return (
    a.addr === b.addr &&
    (a.port ?? DEFAULT_PORT[a.protocol]) === (b.port ?? DEFAULT_PORT[b.protocol])
  );
}

export function selectCollapsed(state: AdapterLiveState): CollapsedAdapter[] {
  return Object.values(state).sort(
    (left, right) => left.startedAtUs - right.startedAtUs,
  );
}

export function findCollapsed(
  state: AdapterLiveState,
  target: AdapterTarget,
): CollapsedAdapter | undefined {
  return selectCollapsed(state).find(record =>
    sameTarget(record.params.target, target),
  );
}

/** Marks a record ended; the picture's end time goes into its history. */
export function endRecord(
  record: CollapsedAdapter,
  error: AdapterErrorInfo | null,
  nowUs: number,
  /** Our own clean-up is not a reason to put in 最近. */
  recordError = true,
): CollapsedAdapter {
  const history = { ...record.history };
  history.stoppedAtUs ??= nowUs;
  if (recordError && error !== null && error.code !== 'cancelled') {
    history.error = error;
  }
  return { ...record, phase: 'ended', error, history };
}

/** adapter.state / adapter.screens for a session that is folded away. */
export function applyAdapterLiveEvent(
  state: AdapterLiveState,
  event: CoreEvent,
  nowUs: number,
): AdapterLiveState {
  if (event.type !== 'adapter.state' && event.type !== 'adapter.screens') {
    return state;
  }
  const record = state[event.sessionId];
  // An ended record keeps the reason it ended with; later events (our own
  // close after a timeout) must not replace it.
  if (record === undefined || record.phase === 'ended') {
    return state;
  }
  if (event.type === 'adapter.screens') {
    return { ...state, [event.sessionId]: { ...record, screens: event.screens } };
  }
  if (event.phase === 'ended') {
    return {
      ...state,
      [event.sessionId]: endRecord(record, event.error ?? null, nowUs),
    };
  }
  return {
    ...state,
    [event.sessionId]: {
      ...record,
      phase: event.phase,
      width: event.width ?? record.width,
      height: event.height ?? record.height,
      serverName: event.serverName ?? record.serverName,
    },
  };
}

function without(state: AdapterLiveState, id: SessionId): AdapterLiveState {
  return Object.fromEntries(
    Object.entries(state).filter(([key]) => key !== id),
  );
}

function createAdapterLiveStore() {
  const store = create<AdapterLiveState>(() => ({}));
  return Object.assign(store, {
    put(record: CollapsedAdapter) {
      store.setState(state => ({ ...state, [record.sessionId]: record }));
    },
    peek(id: SessionId): CollapsedAdapter | undefined {
      return store.getState()[id];
    },
    /** Removes the record and returns it: its page owns the session again. */
    take(id: SessionId): CollapsedAdapter | undefined {
      const record = store.getState()[id];
      if (record !== undefined) {
        store.setState(state => without(state, id), true);
      }
      return record;
    },
    apply(event: CoreEvent, nowUs: number = Date.now() * 1000) {
      store.setState(state => applyAdapterLiveEvent(state, event, nowUs));
    },
    end(
      id: SessionId,
      error: AdapterErrorInfo,
      nowUs: number = Date.now() * 1000,
      recordError = true,
    ) {
      store.setState(state => {
        const record = state[id];
        if (record === undefined || record.phase === 'ended') {
          return state;
        }
        return { ...state, [id]: endRecord(record, error, nowUs, recordError) };
      });
    },
  });
}

export const adapterLive = createAdapterLiveStore();

export function resetAdapterLive(): void {
  adapterLive.setState({}, true);
  shownBy.clear();
}

// which pages show each session right now. A page leaving later
// than the page that picked its session up (a stacked or slow unmount) must
// not fold away and pause a picture another page is showing.
const shownBy = new Map<SessionId, Set<symbol>>();

/** `page` shows `id` until the returned release runs. */
export function showAdapterSession(id: SessionId, page: symbol): () => void {
  const pages = shownBy.get(id) ?? new Set<symbol>();
  pages.add(page);
  shownBy.set(id, pages);
  return () => {
    pages.delete(page);
    if (pages.size === 0 && shownBy.get(id) === pages) {
      shownBy.delete(id);
    }
  };
}

/** Whether a page other than `page` shows `id`. */
export function shownElsewhere(id: SessionId, page: symbol): boolean {
  for (const other of shownBy.get(id) ?? []) {
    if (other !== page) {
      return true;
    }
  }
  return false;
}

export function useAdapterLive<T>(selector: (state: AdapterLiveState) => T): T {
  return useStore(adapterLive, useShallow(selector));
}

/** The route params a record goes back with, without the session id. */
export function routeParamsOf(
  params: RootStackParamList['AdapterSession'],
): AdapterRouteParams {
  const rest = { ...params };
  delete rest.sessionId;
  return rest;
}
