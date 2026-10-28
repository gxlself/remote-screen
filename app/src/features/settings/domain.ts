import type {
  HostPrefs,
  NetworkEntry,
  SettingValue,
  SettingsKey,
} from './schema';

const HOST_PREF_KEYS = new Set([
  'connectVia',
  'routePolicy',
  'tcpFallback',
  'autoUpdateAddr',
  'lockOnEnd',
]);
const NETWORK_ENTRY_KEYS = new Set(['id', 'name', 'cidrs', 'vpnPools']);

export function isPlainObject(
  value: unknown,
): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function isBoolean(value: unknown): boolean {
  return typeof value === 'boolean';
}

function isFiniteNumber(value: unknown): boolean {
  return typeof value === 'number' && Number.isFinite(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(item => typeof item === 'string');
}

export function isNetworkEntry(value: unknown): value is NetworkEntry {
  return (
    isPlainObject(value) &&
    Object.keys(value).length === NETWORK_ENTRY_KEYS.size &&
    Object.keys(value).every(key => NETWORK_ENTRY_KEYS.has(key)) &&
    typeof value.id === 'string' &&
    typeof value.name === 'string' &&
    isStringArray(value.cidrs) &&
    isStringArray(value.vpnPools)
  );
}

export function isHostPrefsPatch(value: unknown): value is Partial<HostPrefs> {
  if (!isPlainObject(value)) {
    return false;
  }
  return Object.keys(value).every(key => {
    if (!HOST_PREF_KEYS.has(key)) {
      return false;
    }
    if (key === 'connectVia') {
      return (
        value[key] === 'auto' || value[key] === 'native' || value[key] === 'host'
      );
    }
    if (key === 'routePolicy') {
      return (
        value[key] === 'auto' ||
        value[key] === 'tailnet' ||
        value[key] === 'intranet'
      );
    }
    return isBoolean(value[key]);
  });
}

const validators: { [K in SettingsKey]: (value: unknown) => boolean } = {
  'connect.prefer': value => value === 'native' || value === 'host',
  'quality.default': value =>
    value === 'auto' ||
    value === 'clear' ||
    value === 'smooth' ||
    value === 'eco',
  'session.mode.phone': value => value === 'trackpad' || value === 'touch',
  'session.mode.tablet': value => value === 'trackpad' || value === 'touch',
  'session.hud': isBoolean,
  'session.coachShown': isBoolean,
  'tailnet.enabled': isBoolean,
  'tailnet.mode': value =>
    value === 'system' || value === 'embedded' || value === 'auto',
  'tailnet.controlUrl': value =>
    value === null ||
    (typeof value === 'string' &&
      value.length <= 512 &&
      /^https?:\/\/[^\s/?#]+/.test(value)),
  'prewarm.enabled': isBoolean,
  'lock.onOpen': isBoolean,
  'lock.beforeSession': isBoolean,
  'lock.sensitiveHosts': isStringArray,
  networks: value => Array.isArray(value) && value.every(isNetworkEntry),
  'onboarding.done': isBoolean,
  'notify.enabled': isBoolean,
  'session.cursorSpeed': isFiniteNumber,
  'session.naturalScroll': isBoolean,
  'session.haptics': isBoolean,
  'session.sound': isBoolean,
  'session.collapsedKeep': value =>
    value === '10m' || value === '30m' || value === '2h' || value === 'always',
  'net.preferLocal': isBoolean,
  'net.warnVpnConflict': isBoolean,
  'terminal.fontSize': isFiniteNumber,
  'terminal.tmux': isBoolean,
  'hosts.prefs': value =>
    isPlainObject(value) &&
    Object.values(value).every(prefs => isHostPrefsPatch(prefs)),
  'clipboard.autoSync': isBoolean,
  'keyboard.cmdAsCtrl': isBoolean,
  'keyboard.pasteSync': isBoolean,
  'recording.maxTotalMb': value =>
    value === 1024 || value === 2048 || value === 4096 || value === 8192,
  'recording.retentionDays': value =>
    value === null || value === 7 || value === 30 || value === 90,
  'recording.introShown': isBoolean,
};

export function isSettingValue<K extends SettingsKey>(
  key: K,
  value: unknown,
): value is SettingValue<K> {
  return validators[key](value);
}
