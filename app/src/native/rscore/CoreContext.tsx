import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { RsCoreApi } from './RsCoreApi';

const CoreContext = createContext<RsCoreApi | null>(null);

export function CoreProvider({
  core,
  children,
}: {
  core: RsCoreApi;
  children: ReactNode;
}) {
  return <CoreContext.Provider value={core}>{children}</CoreContext.Provider>;
}

export function useCore(): RsCoreApi {
  const core = useContext(CoreContext);
  if (core === null) {
    throw new Error('useCore 必须在 CoreProvider 里使用');
  }
  return core;
}
