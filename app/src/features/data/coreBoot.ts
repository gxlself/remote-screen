import type { LaunchOptions } from '../../app/launchOptions';
import { harnessLog } from '../../native/devharness/log';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { PlatformServices } from '../../native/platform/PlatformServices';
import { createSettingsStore, setSettingsStorage } from '../settings/store';
import type { KvStore } from '../storage/kv';
import type { AppServices } from './appServices';
import { createQueryClient } from './queryClient';

const INIT_TIMEOUT_MS = 15000;

type CoreMode =
  | { kind: 'real' }
  | { kind: 'fake'; reason: 'noDataDir' | 'platform' };

export type BootDeps = {
  os: string;
  platform: PlatformServices;
  loadReal: () => RsCoreApi;
  createKv: () => KvStore;
  now: () => number;
};

export class CoreBootError extends Error {
  constructor(
    public kind: RsCoreError['kind'] | 'load' | 'timeout',
    message: string,
  ) {
    super(message);
    this.name = 'CoreBootError';
  }
}

export function selectCoreMode(opts: LaunchOptions, os: string): CoreMode {
  if (os !== 'ios' && os !== 'android') {
    return { kind: 'fake', reason: 'platform' };
  }
  if (!opts.dataDir) {
    return { kind: 'fake', reason: 'noDataDir' };
  }
  return { kind: 'real' };
}

export function bootConfig(
  opts: LaunchOptions,
  info: ReturnType<PlatformServices['deviceInfo']>,
): Parameters<RsCoreApi['init']>[0] {
  return {
    dataDir: opts.dataDir ?? '',
    appVersion: info.appVersion,
    platform: info.isPad ? 'ipados' : info.platform,
  };
}

function makeServices(
  core: RsCoreApi,
  kv: KvStore,
  platform: PlatformServices,
): AppServices {
  const settingsStore = createSettingsStore(kv, core);
  const queryClient = createQueryClient({
    test: process.env.NODE_ENV === 'test',
  });
  return { core, platform, kv, settingsStore, queryClient };
}

function logBoot(core: RsCoreApi, mode: CoreMode, ms: number): void {
  const payload = {
    core: mode.kind,
    ...(mode.kind === 'fake' ? { reason: mode.reason } : {}),
    ms,
  };
  harnessLog('BOOT', payload);
  if (mode.kind === 'real') {
    core.logs.write('info', 'boot', JSON.stringify(payload));
  }
}

export async function bootServices(
  opts: LaunchOptions,
  deps: BootDeps,
): Promise<AppServices> {
  const startedAt = deps.now();
  const mode = selectCoreMode(opts, deps.os);
  let core: RsCoreApi;
  try {
    core =
      mode.kind === 'real' ? deps.loadReal() : createFakeCore();
  } catch (error) {
    throw new CoreBootError(
      'load',
      error instanceof Error ? error.message : String(error),
    );
  }

  const kv = deps.createKv();
  setSettingsStorage(kv);
  const cfg = bootConfig(opts, deps.platform.deviceInfo());
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      core.init(cfg),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          reject(
            new CoreBootError('timeout', 'Core init timed out after 15 s'),
          );
        }, INIT_TIMEOUT_MS);
      }),
    ]);
  } catch (error) {
    if (error instanceof CoreBootError || error instanceof RsCoreError) {
      throw error;
    }
    throw new CoreBootError(
      'internal',
      error instanceof Error ? error.message : String(error),
    );
  } finally {
    clearTimeout(timer);
  }

  const services = makeServices(core, kv, deps.platform);
  logBoot(core, mode, deps.now() - startedAt);
  return services;
}

/** FakeCore.init only records the config, so existing callers can render synchronously. */
export function createFakeServices(
  opts: LaunchOptions,
  deps: BootDeps,
): AppServices {
  const startedAt = deps.now();
  const selected = selectCoreMode(opts, deps.os);
  const mode: Extract<CoreMode, { kind: 'fake' }> =
    selected.kind === 'fake' ? selected : { kind: 'fake', reason: 'platform' };
  const core = createFakeCore();
  const kv = deps.createKv();
  setSettingsStorage(kv);
  const cfg = bootConfig(opts, deps.platform.deviceInfo());
  core.init(cfg);
  const services = makeServices(core, kv, deps.platform);
  const ms = deps.now() - startedAt;
  // Fake services are created during render; let harness readiness effects log first.
  setTimeout(() => logBoot(core, mode, ms), 0);
  return services;
}
