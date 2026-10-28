import { fireEvent, render, screen } from '@testing-library/react-native';
import { Text, Pressable } from 'react-native';
import { act } from '@testing-library/react-native';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { MemoryKv } from '../storage/kv';
import { settingsSchema, toggleSensitive } from './schema';
import type { SettingsKey } from './schema';
import {
  SettingsProvider,
  createSettingsStore,
  useHostPrefs,
  useSetting,
} from './store';

function coreStub() {
  return { settings: { set: jest.fn() } };
}

function HUD() {
  const [hud, setHud] = useSetting('session.hud');
  return (
    <Pressable testID="hud-toggle" onPress={() => setHud(!hud)}>
      <Text testID="hud-value">{String(hud)}</Text>
    </Pressable>
  );
}

function HostPrefs({ hostId }: { hostId: string }) {
  const [prefs, setPrefs] = useHostPrefs(hostId);
  return (
    <>
      <Text testID={`policy-${hostId}`}>{prefs.routePolicy}</Text>
      <Text testID={`fallback-${hostId}`}>{String(prefs.tcpFallback)}</Text>
      <Pressable
        testID={`fallback-toggle-${hostId}`}
        onPress={() => setPrefs({ tcpFallback: !prefs.tcpFallback })}
      />
    </>
  );
}

afterEach(() => {
  jest.restoreAllMocks();
});

describe('settings defaults', () => {
  for (const key of Object.keys(settingsSchema) as SettingsKey[]) {
    it(`uses the default for ${key}`, () => {
      const store = createSettingsStore(new MemoryKv());
      expect(store.get(key)).toEqual(settingsSchema[key].def);
    });
  }
});

it('reads a valid string and rejects a wrongly typed value', () => {
  const kv = new MemoryKv({
    'settings.quality.default': '"smooth"',
    'settings.session.hud': '"yes"',
  });
  const store = createSettingsStore(kv);
  expect(store.get('quality.default')).toBe('smooth');
  expect(store.get('session.hud')).toBe(false);
});

it('rejects malformed arrays and host preference records', () => {
  const kv = new MemoryKv({
    'settings.networks': '[{"id":"net-1"}]',
    'settings.lock.sensitiveHosts': '["h1",1]',
    'settings.hosts.prefs': '{"h1":"not-prefs"}',
  });
  const store = createSettingsStore(kv);
  expect(store.get('networks')).toEqual([]);
  expect(store.get('lock.sensitiveHosts')).toEqual([]);
  expect(store.get('hosts.prefs')).toEqual({});
});

it('returns copies of arrays and objects from get', () => {
  const store = createSettingsStore(new MemoryKv());
  const networks = store.get('networks');
  networks.push({
    id: 'net-1',
    name: 'Office',
    cidrs: ['10.30.0.0/16'],
    vpnPools: ['10.99.0.0/16'],
  });
  expect(store.get('networks')).toEqual([]);

  const prefs = store.get('hosts.prefs');
  prefs.h1 = { tcpFallback: false };
  expect(store.get('hosts.prefs')).toEqual({});
});

it('updates a component and persists non-synced values locally', async () => {
  const kv = new MemoryKv();
  const core = coreStub();
  const store = createSettingsStore(kv, core);
  core.settings.set.mockClear();

  await render(
    <SettingsProvider store={store}>
      <HUD />
    </SettingsProvider>,
  );
  expect(screen.getByTestId('hud-value')).toHaveTextContent('false');

  await fireEvent.press(screen.getByTestId('hud-toggle'));
  expect(screen.getByTestId('hud-value')).toHaveTextContent('true');
  expect(kv.getString('settings.session.hud')).toBe('true');
  expect(core.settings.set).not.toHaveBeenCalled();
});

it('pushes synced settings to the core', () => {
  const kv = new MemoryKv();
  const core = coreStub();
  const store = createSettingsStore(kv, core);
  core.settings.set.mockClear();

  store.set('quality.default', 'eco');

  expect(core.settings.set).toHaveBeenLastCalledWith('quality.default', 'eco');
  expect(kv.getString('settings.quality.default')).toBe('"eco"');
});

it('saves locally and warns once when core settings are unsupported', () => {
  const kv = new MemoryKv();
  const warn = jest.spyOn(console, 'warn').mockImplementation(() => {});
  const core = coreStub();
  core.settings.set.mockImplementation(() => {
    throw new RsCoreError('unsupported', 'x');
  });
  const store = createSettingsStore(kv, core);

  expect(() => store.set('prewarm.enabled', false)).not.toThrow();
  expect(store.get('prewarm.enabled')).toBe(false);
  expect(kv.getString('settings.prewarm.enabled')).toBe('false');

  store.set('quality.default', 'eco');
  expect(warn).toHaveBeenCalledTimes(1);
});

it('reads saved settings normally when removed Hub keys are left in storage', () => {
  const kv = new MemoryKv();
  kv.setJSON('settings.hub.url', 'https://hs.example.com/hub/');
  kv.setJSON('settings.crash.upload', true);
  kv.setJSON('settings.session.hud', true);
  kv.setJSON('settings.quality.default', 'eco');
  const core = coreStub();

  const store = createSettingsStore(kv, core);

  expect(store.get('session.hud')).toBe(true);
  expect(store.get('quality.default')).toBe('eco');
  expect(store.getState()).not.toHaveProperty('hub.url');
  expect(store.getState()).not.toHaveProperty('crash.upload');
  const pushed = core.settings.set.mock.calls.map(([key]) => key);
  expect(pushed).not.toContain('hub.url');
  expect(pushed).not.toContain('crash.upload');
  expect(kv.getString('settings.hub.url')).toBe(
    '"https://hs.example.com/hub/"',
  );
});

it('pushes exactly the eight synced keys while creating the store', () => {
  const core = coreStub();
  createSettingsStore(new MemoryKv(), core);
  expect(core.settings.set).toHaveBeenCalledTimes(8);
  expect(core.settings.set.mock.calls.map(([key]) => key)).toEqual([
    'quality.default',
    'tailnet.enabled',
    'tailnet.mode',
    'tailnet.controlUrl',
    'prewarm.enabled',
    'networks',
    'recording.maxTotalMb',
    'recording.retentionDays',
  ]);
  expect(core.settings.set).toHaveBeenCalledWith('networks', []);
});

it('keeps the core on the system tailnet while Tailscale is switched off', () => {
  const core = coreStub();
  const store = createSettingsStore(
    new MemoryKv({ 'settings.tailnet.mode': '"embedded"' }),
    core,
  );
  expect(core.settings.set).toHaveBeenCalledWith('tailnet.mode', 'system');
  expect(store.get('tailnet.mode')).toBe('embedded');

  core.settings.set.mockClear();
  store.set('tailnet.enabled', true);
  expect(core.settings.set.mock.calls).toEqual([
    ['tailnet.enabled', true],
    ['tailnet.mode', 'embedded'],
  ]);

  core.settings.set.mockClear();
  store.set('tailnet.enabled', false);
  expect(core.settings.set).toHaveBeenLastCalledWith('tailnet.mode', 'system');
});

it('pushes network entries to the core when they change', () => {
  const core = coreStub();
  const store = createSettingsStore(new MemoryKv(), core);
  core.settings.set.mockClear();

  const network = {
    id: 'net-1',
    name: 'Office',
    cidrs: ['10.30.0.0/16'],
    vpnPools: ['10.99.0.0/16'],
  };
  store.set('networks', [network]);

  expect(core.settings.set).toHaveBeenCalledTimes(1);
  expect(core.settings.set).toHaveBeenCalledWith('networks', [network]);
});

it('merges host preferences independently', async () => {
  const kv = new MemoryKv();
  const store = createSettingsStore(kv);

  await render(
    <SettingsProvider store={store}>
      <HostPrefs hostId="h1" />
      <HostPrefs hostId="h2" />
    </SettingsProvider>,
  );
  expect(screen.getByTestId('policy-h1')).toHaveTextContent('auto');

  await fireEvent.press(screen.getByTestId('fallback-toggle-h1'));
  expect(screen.getByTestId('fallback-h1')).toHaveTextContent('false');
  expect(screen.getByTestId('policy-h1')).toHaveTextContent('auto');
  expect(screen.getByTestId('fallback-h2')).toHaveTextContent('true');
  expect(store.get('hosts.prefs')).toEqual({ h1: { tcpFallback: false } });
});

describe('toggleSensitive', () => {
  it('adds a host without duplicates', () => {
    expect(toggleSensitive(['a'], 'b', true)).toEqual(['a', 'b']);
    expect(toggleSensitive(['a', 'b'], 'b', true)).toEqual(['a', 'b']);
  });

  it('removes a host', () => {
    expect(toggleSensitive(['a', 'b'], 'a', false)).toEqual(['b']);
  });
});

it('exposes get and set through the settings state for non-component code', async () => {
  const store = createSettingsStore(new MemoryKv());
  await act(() => store.getState().set('session.haptics', false));
  expect(store.getState().get('session.haptics')).toBe(false);
});

it('falls back from an invalid stored quality and pushes the default', () => {
  const core = coreStub();
  const store = createSettingsStore(
    new MemoryKv({ 'settings.quality.default': '"ultra"' }),
    core,
  );

  expect(store.get('quality.default')).toBe('auto');
  expect(core.settings.set).toHaveBeenCalledWith('quality.default', 'auto');
});

it('falls back from invalid enum, number and network entries on disk', () => {
  const store = createSettingsStore(
    new MemoryKv({
      'settings.hosts.prefs': '{"h1":{"routePolicy":"lan"}}',
      'settings.session.cursorSpeed': '1e999',
      'settings.networks':
        '[{"id":"n","name":"x","cidrs":[],"vpnPools":[],"extra":true}]',
    }),
  );

  expect(store.get('hosts.prefs')).toEqual({});
  expect(store.get('session.cursorSpeed')).toBe(1.4);
  expect(store.get('networks')).toEqual([]);
});
