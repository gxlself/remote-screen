import { createContext, useContext, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { useCore } from '../../native/rscore/CoreContext';
import type {
  CoreEvent,
  CoreEventOf,
  CoreEventType,
} from '../../native/rscore/types';
import { createEventBus } from './bus';
import type { EventBus } from './bus';
import { NetworkStoreProvider, createNetworkStore } from '../network/store';
import type { NetworkStore } from '../network/store';
import {
  applyWorkProfile,
  subscribeWorkProfile,
} from '../../native/netinfo/workProfile';
import {
  PathStoreProvider,
  applyPathEvent,
  createPathStore,
} from '../directory/pathStore';
import type { PathStore } from '../directory/pathStore';
import { attachPathPersistence, loadPaths } from '../directory/pathPersist';
import { useOptionalKv } from '../storage/StorageContext';
import {
  SessionsStoreProvider,
  applySessionEvent,
  createSessionsStore,
} from '../session/store';
import type { SessionsStore } from '../session/store';

const EventsContext = createContext<EventBus | null>(null);

/** Everything EventsProvider hands its tree. */
export type EventsValue = {
  bus: EventBus;
  sessionsStore: SessionsStore;
  networkStore: NetworkStore;
  pathStore: PathStore;
};

const EventsValueContext = createContext<EventsValue | null>(null);

/** another window's React tree gets the same bus and stores. */
export function useEventsValue(): EventsValue | null {
  return useContext(EventsValueContext);
}

export function EventsValueProvider({
  value,
  children,
}: {
  value: EventsValue;
  children: ReactNode;
}) {
  return (
    <SessionsStoreProvider store={value.sessionsStore}>
      <NetworkStoreProvider store={value.networkStore}>
        <PathStoreProvider store={value.pathStore}>
          <EventsValueContext.Provider value={value}>
            <EventsContext.Provider value={value.bus}>
              {children}
            </EventsContext.Provider>
          </EventsValueContext.Provider>
        </PathStoreProvider>
      </NetworkStoreProvider>
    </SessionsStoreProvider>
  );
}

function useEventBus(): EventBus {
  const bus = useContext(EventsContext);
  if (bus === null) {
    throw new Error('useCoreEvent 必须在 EventsProvider 里使用');
  }
  return bus;
}

function useOptionalCore(): RsCoreApi | null {
  try {
    return useCore();
  } catch {
    return null;
  }
}

export function useCoreEvent<T extends CoreEventType>(
  type: T,
  handler: (event: CoreEventOf<T>) => void,
): void;
export function useCoreEvent(
  type: '*',
  handler: (event: CoreEvent) => void,
): void;
export function useCoreEvent<T extends CoreEventType>(
  type: T | '*',
  handler: (event: CoreEvent) => void,
): void {
  const bus = useEventBus();
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    if (type === '*') {
      return bus.on('*', event => handlerRef.current(event));
    }
    return bus.on(type as CoreEventType, event => handlerRef.current(event));
  }, [bus, type]);
}

export default function EventsProvider({ children }: { children: ReactNode }) {
  const core = useOptionalCore();
  const kv = useOptionalKv();
  const [bus] = useState(createEventBus);
  const [sessionsStore] = useState(createSessionsStore);
  const [networkStore] = useState(createNetworkStore);
  const [pathStore] = useState(() =>
    kv === null
      ? createPathStore()
      : createPathStore({ byHost: loadPaths(kv, Date.now() * 1000) }),
  );

  useEffect(
    () => (kv === null ? undefined : attachPathPersistence(pathStore, kv)),
    [kv, pathStore],
  );

  useEffect(() => {
    if (core === null) {
      return;
    }

    const unsubscribe = core.events.subscribe(event => {
      const nowUs = Date.now() * 1000;
      sessionsStore.setState(state => applySessionEvent(state, event, nowUs));
      pathStore.setState(state =>
        applyPathEvent(state, event, {
          nowUs,
          hostIdOf: sessionId => sessionsStore.getState()[sessionId]?.hostId,
        }),
      );
      if (event.type === 'network.changed') {
        networkStore.setState({
          status: applyWorkProfile(event.status),
          updatedAtUs: nowUs,
        });
      }
      bus.emit(event);
    });

    try {
      sessionsStore.seed(core.sessions.list());
    } catch {
      // A missing sessions list must not take down the rest of the event bus.
    }

    let active = true;
    try {
      const pending = core.network.status();
      Promise.resolve(pending)
        .then(status => {
          if (!active) {
            return;
          }
          networkStore.setState(state =>
            state.updatedAtUs === null
              ? { status, updatedAtUs: Date.now() * 1000 }
              : state,
          );
        })
        .catch(() => {
          // Network status is optional; a live network.changed event can fill it.
        });
    } catch {
      // Synchronous unsupported implementations keep the store at null.
    }

    const unsubscribeWorkProfile = subscribeWorkProfile(() => {
      networkStore.setState(state =>
        state.status === null
          ? state
          : { ...state, status: applyWorkProfile(state.status) },
      );
    });

    return () => {
      active = false;
      unsubscribe();
      unsubscribeWorkProfile();
    };
  }, [bus, core, networkStore, pathStore, sessionsStore]);

  const [value] = useState<EventsValue>(() => ({
    bus,
    sessionsStore,
    networkStore,
    pathStore,
  }));

  return <EventsValueProvider value={value}>{children}</EventsValueProvider>;
}

export { selectLive, useSessions, useSessionsActions } from '../session/store';
export { useNetwork } from '../network/store';
export { useHostPath, useHostRoutes } from '../directory/pathStore';
export type { SessionsState, SessionRecord } from '../session/store';
export type { NetworkState } from '../network/store';
export type { PathState, RouteResult } from '../directory/pathStore';
