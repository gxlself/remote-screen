import { Platform } from 'react-native';
import type { QueryClient } from '@tanstack/react-query';
import { getLaunchOptions } from '../../app/launchOptions';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { loadRealCore } from '../../native/rscore/realCore';
import { createRnPlatform } from '../../native/platform/rnPlatform';
import type { PlatformServices } from '../../native/platform/PlatformServices';
import { createMmkvKv } from '../storage/mmkvKv';
import type { KvStore } from '../storage/kv';
import type { SettingsStore } from '../settings/store';
import { createFakeServices } from './coreBoot';
import type { BootDeps } from './coreBoot';

export type AppServices = {
  core: RsCoreApi;
  platform: PlatformServices;
  kv: KvStore;
  settingsStore: SettingsStore;
  queryClient: QueryClient;
};

export const defaultDeps: BootDeps = {
  os: Platform.OS,
  platform: createRnPlatform(),
  loadReal: loadRealCore,
  createKv: createMmkvKv,
  now: Date.now,
};

export function createAppServices(): AppServices {
  return createFakeServices(getLaunchOptions(), defaultDeps);
}
