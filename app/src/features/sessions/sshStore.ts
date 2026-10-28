import { createContext, createElement, useContext } from 'react';
import type { ReactNode } from 'react';
import { create, useStore } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import type { StoreApi, UseBoundStore } from 'zustand';
import type {
  CoreEvent,
  CoreEventOf,
  Micros,
  SshId,
} from '../../native/rscore/types';

export type SshRecord = {
  sshId: SshId;
  serverId: string;
  state: CoreEventOf<'ssh.state'>['state'];
  openedAtUs: number | null;
  updatedAtUs: number;
  message?: string;
};

export type SshState = Record<SshId, SshRecord>;

export const LIVE_SSH_STATES = [
  'connecting',
  'authenticating',
  'open',
] as const;

export type SshStoreApi = UseBoundStore<StoreApi<SshState>> & {
  apply(event: CoreEvent, nowUs?: Micros): void;
  remove(sshId: SshId): void;
};

type LiveSshState = (typeof LIVE_SSH_STATES)[number];

function isLive(state: SshRecord['state']): state is LiveSshState {
  return (LIVE_SSH_STATES as readonly SshRecord['state'][]).includes(state);
}

export function applySshEvent(
  state: SshState,
  event: CoreEvent,
  nowUs: number,
): SshState {
  if (event.type !== 'ssh.state') {
    return state;
  }

  const current = state[event.sshId];
  if (current === undefined) {
    const record: SshRecord = {
      sshId: event.sshId,
      serverId: event.serverId,
      state: event.state,
      openedAtUs: event.state === 'open' ? nowUs : null,
      updatedAtUs: nowUs,
    };
    if (event.message !== undefined) {
      record.message = event.message;
    }
    return { ...state, [event.sshId]: record };
  }

  const next: SshRecord = {
    ...current,
    serverId: event.serverId,
    state: event.state,
    updatedAtUs: nowUs,
  };
  if (next.openedAtUs === null && event.state === 'open') {
    next.openedAtUs = nowUs;
  }
  if (event.message !== undefined) {
    next.message = event.message;
  }
  return { ...state, [event.sshId]: next };
}

export function selectLiveSsh(state: SshState): SshRecord[] {
  return Object.values(state)
    .filter(record => isLive(record.state))
    .sort(
      (left, right) =>
        (left.openedAtUs ?? left.updatedAtUs) -
        (right.openedAtUs ?? right.updatedAtUs),
    );
}

export function createSshStore(): SshStoreApi {
  const store = create<SshState>(() => ({}));

  return Object.assign(store, {
    apply(event: CoreEvent, nowUs: Micros = Date.now() * 1000) {
      store.setState(state => applySshEvent(state, event, nowUs));
    },
    remove(sshId: SshId) {
      store.setState(state => {
        if (state[sshId] === undefined) {
          return state;
        }
        return Object.fromEntries(
          Object.entries(state).filter(([key]) => key !== sshId),
        );
      }, true);
    },
  });
}

export const sshStore = createSshStore();

export function resetSshStore(): void {
  sshStore.setState({}, true);
}

export const SshStoreContext = createContext<SshStoreApi | null>(null);

export function SshStoreProvider({
  store,
  children,
}: {
  store: SshStoreApi;
  children: ReactNode;
}) {
  return createElement(SshStoreContext.Provider, { value: store }, children);
}

function useScopedSshStore(): SshStoreApi | null {
  return useContext(SshStoreContext);
}

export function useSshSessions<T>(selector: (state: SshState) => T): T {
  const scoped = useScopedSshStore();
  return useStore(scoped ?? sshStore, useShallow(selector));
}
