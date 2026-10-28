import { parseLaunchProps, setLaunchOptions } from '../../app/launchOptions';
import { harnessLog } from '../../native/devharness/log';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import * as fakeCoreModule from '../../native/rscore/fakeCore';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { MemoryKv } from '../storage/kv';
import { setSettingsStorage } from '../settings/store';
import {
  bootConfig,
  bootServices,
  createFakeServices,
  selectCoreMode,
} from './coreBoot';

jest.mock('../../native/devharness/log', () => ({ harnessLog: jest.fn() }));

const realOptions = parseLaunchProps({ dataDir: '/app/support/core' });

function setup() {
  const core = createFakeCore();
  const platform = createFakePlatform();
  const deps = {
    os: 'ios',
    platform,
    loadReal: jest.fn(() => core),
    createKv: jest.fn(() => new MemoryKv()),
    now: jest.fn().mockReturnValueOnce(100).mockReturnValue(125),
  };
  return { core, platform, deps };
}

beforeEach(() => {
  jest.clearAllMocks();
  setLaunchOptions(parseLaunchProps({}));
  setSettingsStorage(new MemoryKv());
});
afterEach(() => jest.useRealTimers());

it('selects real Core for iOS with a data directory', () => {
  expect(selectCoreMode(realOptions, 'ios')).toEqual({ kind: 'real' });
});

it('ignores the removed fake directory launch properties', () => {
  expect(
    selectCoreMode(
      parseLaunchProps({
        dataDir: '/app/support/core',
        fakeDirectoryJson: '{"computers":[],"servers":[]}',
        fakeDirectoryError: 'notFound',
      }),
      'android',
    ),
  ).toEqual({ kind: 'real' });
  expect(harnessLog).not.toHaveBeenCalled();
});

it('selects fake Core without a data directory', () => {
  expect(selectCoreMode(parseLaunchProps({}), 'ios')).toEqual({
    kind: 'fake',
    reason: 'noDataDir',
  });
});

it('selects real Core for Android with a data directory', () => {
  expect(selectCoreMode(realOptions, 'android')).toEqual({ kind: 'real' });
});

it('selects fake Core for Android without a data directory', () => {
  expect(selectCoreMode(parseLaunchProps({}), 'android')).toEqual({
    kind: 'fake',
    reason: 'noDataDir',
  });
});

it('selects fake Core on platforms without rs-mobile', () => {
  expect(selectCoreMode(realOptions, 'windows')).toEqual({
    kind: 'fake',
    reason: 'platform',
  });
});

it('builds the default config and maps iPad to ipados', () => {
  const { platform } = setup();
  expect(
    bootConfig(parseLaunchProps({}), { ...platform.info, isPad: true }),
  ).toEqual({
    dataDir: '',
    appVersion: platform.info.appVersion,
    platform: 'ipados',
  });
});

it('does not pass a Hub URL to init', async () => {
  const { core, deps } = setup();
  await bootServices(
    parseLaunchProps({
      dataDir: '/app/support/core',
      hubUrl: 'http://100.64.0.10:7300',
    }),
    deps,
  );
  expect(core.calls[0]?.method).toBe('init');
  expect(core.calls[0]?.args[0]).not.toHaveProperty('hubUrl');
});

it('ignores a Hub address an older version left in MMKV', async () => {
  const { core, deps } = setup();
  const kv = new MemoryKv();
  kv.setJSON('settings.hub.url', 'https://hs.example.com/hub/');
  deps.createKv.mockReturnValue(kv);
  const services = await bootServices(realOptions, deps);
  expect(core.calls[0]?.method).toBe('init');
  expect(core.calls[0]?.args[0]).not.toHaveProperty('hubUrl');
  expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(false);
  expect(services.settingsStore.getState()).not.toHaveProperty('hub.url');
});

it('initializes with the correct config before syncing settings and logs without dataDir', async () => {
  const { core, deps, platform } = setup();
  const services = await bootServices(realOptions, deps);
  expect(services.core).toBe(core);
  expect(core.calls[0]).toEqual({
    method: 'init',
    args: [
      {
        dataDir: realOptions.dataDir,
        appVersion: platform.info.appVersion,
        platform: 'ios',
      },
    ],
  });
  expect(
    core.calls.findIndex(call => call.method === 'settings.set'),
  ).toBeGreaterThan(0);
  const payload = { core: 'real', ms: 25 };
  expect(harnessLog).toHaveBeenCalledWith('BOOT', payload);
  expect(core.calls.find(call => call.method === 'logs.write')?.args).toEqual([
    'info',
    'boot',
    JSON.stringify(payload),
  ]);
});

it('reads storage before init and waits for init before syncing settings', async () => {
  const { core, deps } = setup();
  let complete!: () => void;
  jest.spyOn(core, 'init').mockImplementation(
    () =>
      new Promise<void>(resolve => {
        complete = resolve;
      }),
  );
  const pending = bootServices(realOptions, deps);
  expect(deps.createKv).toHaveBeenCalledTimes(1);
  expect(core.calls).toEqual([]);
  complete();
  await pending;
  expect(deps.createKv).toHaveBeenCalledTimes(1);
  expect(core.calls.some(call => call.method === 'settings.set')).toBe(true);
});

it('propagates init RsCoreError kind and does not sync settings on failure', async () => {
  const { core, deps } = setup();
  const error = new RsCoreError('network', 'offline');
  core.failNext('init', error);
  await expect(bootServices(realOptions, deps)).rejects.toBe(error);
  expect(deps.createKv).toHaveBeenCalledTimes(1);
  expect(core.calls.map(call => call.method)).toEqual(['init']);
});

it('reports native loading errors without falling back to fake Core', async () => {
  const { deps } = setup();
  deps.loadReal.mockImplementation(() => {
    throw new Error('bindings missing');
  });
  await expect(bootServices(realOptions, deps)).rejects.toMatchObject({
    kind: 'load',
    message: 'bindings missing',
  });
  expect(deps.createKv).not.toHaveBeenCalled();
});

it('normalizes unexpected init errors', async () => {
  const { core, deps } = setup();
  core.failNext('init', new Error('unexpected'));
  await expect(bootServices(realOptions, deps)).rejects.toMatchObject({
    kind: 'internal',
    message: 'unexpected',
  });
});

it('times out after 15 seconds and never syncs settings after a late init', async () => {
  jest.useFakeTimers();
  const { core, deps } = setup();
  let complete!: () => void;
  jest.spyOn(core, 'init').mockImplementation(
    () =>
      new Promise<void>(resolve => {
        complete = resolve;
      }),
  );
  const result = bootServices(realOptions, deps).catch(error => error);
  await jest.advanceTimersByTimeAsync(15000);
  await expect(result).resolves.toMatchObject({ kind: 'timeout' });
  complete();
  await Promise.resolve();
  expect(deps.createKv).toHaveBeenCalledTimes(1);
  expect(core.calls).toEqual([]);
  expect(jest.getTimerCount()).toBe(0);
});

it('clears the timeout when init succeeds', async () => {
  jest.useFakeTimers();
  const { deps } = setup();
  await bootServices(realOptions, deps);
  expect(jest.getTimerCount()).toBe(0);
});

it('boots fake fixtures asynchronously without loading native bindings', async () => {
  const { deps } = setup();
  const create = jest.spyOn(fakeCoreModule, 'createFakeCore');
  await bootServices(parseLaunchProps({}), deps);
  expect(deps.loadReal).not.toHaveBeenCalled();
  expect(create).toHaveBeenCalledWith();
  create.mockRestore();
  expect(harnessLog).toHaveBeenCalledWith('BOOT', {
    core: 'fake',
    reason: 'noDataDir',
    ms: 25,
  });
});

it('creates fake services synchronously with init recorded before settings.set', () => {
  jest.useFakeTimers();
  const { deps } = setup();
  const services = createFakeServices(parseLaunchProps({}), deps);
  const core = services.core as ReturnType<typeof createFakeCore>;
  expect(core.calls[0].method).toBe('init');
  expect(
    core.calls.findIndex(call => call.method === 'settings.set'),
  ).toBeGreaterThan(0);
  expect(deps.loadReal).not.toHaveBeenCalled();
  jest.runOnlyPendingTimers();
  expect(harnessLog).toHaveBeenCalledWith('BOOT', {
    core: 'fake',
    reason: 'noDataDir',
    ms: 25,
  });
});
