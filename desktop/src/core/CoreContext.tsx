import { createContext, useContext, useMemo, type JSX, type ReactNode } from 'react';

import type { RsCoreApi } from '@app/native/rscore/RsCoreApi';

import type { DesktopApi } from './desktopApi';

type Cores = { core: RsCoreApi; desktop: DesktopApi };

const CoreContext = createContext<Cores | null>(null);

export function CoreProvider({
  core,
  desktop,
  children,
}: {
  core: RsCoreApi;
  desktop: DesktopApi;
  children: ReactNode;
}): JSX.Element {
  const value = useMemo(() => ({ core, desktop }), [core, desktop]);
  return <CoreContext.Provider value={value}>{children}</CoreContext.Provider>;
}

function useCores(): Cores {
  const value = useContext(CoreContext);
  if (value === null) {
    throw new Error('useCore / useDesktop 必须在 CoreProvider 里使用');
  }
  return value;
}

export function useCore(): RsCoreApi {
  return useCores().core;
}

export function useDesktop(): DesktopApi {
  return useCores().desktop;
}
