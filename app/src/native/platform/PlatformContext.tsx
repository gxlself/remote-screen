import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { PlatformServices } from './PlatformServices';

const PlatformContext = createContext<PlatformServices | null>(null);

export function PlatformProvider({
  platform,
  children,
}: {
  platform: PlatformServices;
  children: ReactNode;
}) {
  return (
    <PlatformContext.Provider value={platform}>
      {children}
    </PlatformContext.Provider>
  );
}

export function usePlatform(): PlatformServices {
  const platform = useContext(PlatformContext);
  if (platform === null) {
    throw new Error('usePlatform 必须在 PlatformProvider 里使用');
  }
  return platform;
}
