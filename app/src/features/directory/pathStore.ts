import { createContext, createElement, useContext, useRef } from 'react';
import type { ReactNode } from 'react';
import { create, useStore } from 'zustand';
import type { StoreApi, UseBoundStore } from 'zustand';
import type {
  CoreEvent,
  Micros,
  NetPath,
  ProbeOutcome,
  SessionId,
  Transport,
} from '../../native/rscore/types';

export type RouteResult = {
  path: NetPath;
  addr: string;
  port: number;
  transport: Transport;
  outcome: ProbeOutcome;
  ms: number | null;
  reason?: string;
  atUs: Micros;
};

export type HostPath = {
  path: NetPath;
  latencyMs: number;
  transport: Transport;
  derpRegion?: string;
  atUs: Micros;
};

export type PathState = {
  byHost: Record<string, HostPath>;
  routes: Record<string, Record<string, RouteResult>>;
};

const SIXTY_SECONDS_US = 60_000_000;
export function applyPathEvent(
  state: PathState,
  event: CoreEvent,
  ctx: { hostIdOf(sessionId: SessionId): string | undefined; nowUs: Micros },
): PathState {
  switch (event.type) {
    case 'session.stats': {
      const hostId = ctx.hostIdOf(event.sessionId);
      if (hostId === undefined) {
        return state;
      }
      const next: HostPath = {
        path: event.path,
        latencyMs: event.rttMs,
        transport: event.transport,
        atUs: ctx.nowUs,
      };
      if (event.derpRegion !== undefined) {
        next.derpRegion = event.derpRegion;
      }
      return { ...state, byHost: { ...state.byHost, [hostId]: next } };
    }
    case 'probe.result': {
      if (event.hostId === null || event.hostId === '') {
        return state;
      }
      const hostId = event.hostId;
      const key = `${event.addr}:${event.port}`;
      const result: RouteResult = {
        path: event.path,
        addr: event.addr,
        port: event.port,
        transport: event.transport,
        outcome: event.outcome,
        ms: event.ms,
        atUs: ctx.nowUs,
      };
      if (event.reason !== undefined) {
        result.reason = event.reason;
      }
      const routes = {
        ...state.routes,
        [hostId]: { ...state.routes[hostId], [key]: result },
      };

      const previous = state.byHost[hostId];
      const stale =
        previous === undefined || ctx.nowUs - previous.atUs > SIXTY_SECONDS_US;
      const faster =
        previous !== undefined &&
        event.ms !== null &&
        event.ms < previous.latencyMs;
      if (event.outcome !== 'ok' || event.ms === null || (!stale && !faster)) {
        return { ...state, routes };
      }

      const next: HostPath = {
        path: event.path,
        latencyMs: event.ms,
        transport: event.transport,
        atUs: ctx.nowUs,
      };
      if (previous?.derpRegion !== undefined) {
        next.derpRegion = previous.derpRegion;
      }
      return {
        byHost: { ...state.byHost, [hostId]: next },
        routes,
      };
    }
    default:
      return state;
  }
}

export type PathStore = UseBoundStore<StoreApi<PathState>>;

export function createPathStore(initial?: Partial<PathState>): PathStore {
  return create<PathState>(() => ({
    byHost: initial?.byHost ?? {},
    routes: initial?.routes ?? {},
  }));
}

const PathContext = createContext<PathStore | null>(null);

export function PathStoreProvider(props: {
  store: PathStore;
  children: ReactNode;
}) {
  return createElement(
    PathContext.Provider,
    { value: props.store },
    props.children,
  );
}

function usePathStore(): PathStore {
  const store = useContext(PathContext);
  if (store === null) {
    throw new Error('useHostPath 必须在 EventsProvider 里使用');
  }
  return store;
}

export function useHostPath(hostId: string): HostPath | undefined {
  return useStore(usePathStore(), state => state.byHost[hostId]);
}

export function useHostRoutes(hostId: string): Record<string, RouteResult> {
  const routes = useStore(usePathStore(), state => state.routes[hostId]);
  const empty = useRef<Record<string, RouteResult>>({});
  return routes ?? empty.current;
}
