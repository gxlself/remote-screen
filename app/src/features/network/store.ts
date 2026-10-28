import { createContext, createElement, useContext } from 'react';
import type { ReactNode } from 'react';
import { create, useStore } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import type { StoreApi, UseBoundStore } from 'zustand';
import type { Micros, NetworkStatus } from '../../native/rscore/types';

export type NetworkState = {
  status: NetworkStatus | null;
  updatedAtUs: Micros | null;
};

export type NetworkStore = UseBoundStore<StoreApi<NetworkState>>;

export function createNetworkStore(): NetworkStore {
  return create<NetworkState>(() => ({ status: null, updatedAtUs: null }));
}

const NetworkContext = createContext<NetworkStore | null>(null);

export function NetworkStoreProvider(props: {
  store: NetworkStore;
  children: ReactNode;
}) {
  return createElement(
    NetworkContext.Provider,
    { value: props.store },
    props.children,
  );
}

function useNetworkStore(): NetworkStore {
  const store = useContext(NetworkContext);
  if (store === null) {
    throw new Error('useNetwork 必须在 EventsProvider 里使用');
  }
  return store;
}

export function useNetwork<T>(selector: (state: NetworkState) => T): T {
  return useStore(useNetworkStore(), useShallow(selector));
}
