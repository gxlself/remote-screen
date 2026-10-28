import { createContext, useContext, useMemo, type ReactNode } from 'react';

import { FakePlatform } from './fake';
import { createTauriPlatform } from './tauri';
import { isTauri, type Platform } from './types';

const PlatformContext = createContext<Platform | null>(null);

export function PlatformProvider({
  platform,
  children,
}: {
  platform: Platform;
  children: ReactNode;
}) {
  return <PlatformContext.Provider value={platform}>{children}</PlatformContext.Provider>;
}

export function usePlatform(): Platform {
  const platform = useContext(PlatformContext);
  if (platform === null) {
    throw new Error('usePlatform must be used inside a PlatformProvider');
  }
  return platform;
}

export function PlatformRoot({ children }: { children: ReactNode }) {
  const platform = useMemo<Platform>(
    () => (isTauri() ? createTauriPlatform() : new FakePlatform()),
    [],
  );
  return <PlatformProvider platform={platform}>{children}</PlatformProvider>;
}
