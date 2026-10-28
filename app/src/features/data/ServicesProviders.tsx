import type { ReactNode } from 'react';
import { QueryClientProvider } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import { CoreProvider } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { PlatformProvider } from '../../native/platform/PlatformContext';
import type { PlatformServices } from '../../native/platform/PlatformServices';
import { StorageProvider } from '../storage/StorageContext';
import type { KvStore } from '../storage/kv';
import { SettingsProvider } from '../settings/store';
import type { SettingsStore } from '../settings/store';

export function ServicesProviders({
  core,
  platform,
  kv,
  settingsStore,
  queryClient,
  children,
}: {
  core: RsCoreApi;
  platform: PlatformServices;
  kv: KvStore;
  settingsStore: SettingsStore;
  queryClient: QueryClient;
  children: ReactNode;
}) {
  return (
    <CoreProvider core={core}>
      <PlatformProvider platform={platform}>
        <StorageProvider kv={kv}>
          <SettingsProvider store={settingsStore}>
            <QueryClientProvider client={queryClient}>
              {children}
            </QueryClientProvider>
          </SettingsProvider>
        </StorageProvider>
      </PlatformProvider>
    </CoreProvider>
  );
}
