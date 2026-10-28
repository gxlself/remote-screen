import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { KvStore } from './kv';

const StorageContext = createContext<KvStore | null>(null);

export function StorageProvider({
  kv,
  children,
}: {
  kv: KvStore;
  children: ReactNode;
}) {
  return (
    <StorageContext.Provider value={kv}>{children}</StorageContext.Provider>
  );
}

export function useKv(): KvStore {
  const kv = useContext(StorageContext);
  if (kv === null) {
    throw new Error('useKv 必须在 StorageProvider 里使用');
  }
  return kv;
}

/** For providers that also mount in trees without a StorageProvider. */
export function useOptionalKv(): KvStore | null {
  return useContext(StorageContext);
}
