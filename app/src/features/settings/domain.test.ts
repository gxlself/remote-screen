import {
  isHostPrefsPatch,
  isNetworkEntry,
  isPlainObject,
  isSettingValue,
} from './domain';
import { settingsSchema } from './schema';
import type { SettingsKey } from './schema';

describe('isSettingValue', () => {
  it('accepts null and http(s) tailnet control URLs up to 512 characters', () => {
    for (const value of [
      null,
      'http://hs.example.com',
      'https://hs.example.com/base',
      `https://hs.example.com/${'a'.repeat(489)}`,
    ]) {
      expect(isSettingValue('tailnet.controlUrl', value)).toBe(true);
    }
  });

  it('rejects invalid or oversized tailnet control URLs', () => {
    for (const value of [
      '',
      'hs.example.com',
      'ftp://hs.example.com',
      'https://',
      `https://hs.example.com/${'a'.repeat(490)}`,
      true,
      1,
      undefined,
    ]) {
      expect(isSettingValue('tailnet.controlUrl', value)).toBe(false);
    }
  });

  it.each(Object.keys(settingsSchema) as SettingsKey[])(
    'accepts the schema default for %s',
    key => {
      expect(isSettingValue(key, settingsSchema[key].def)).toBe(true);
    },
  );

  const invalidCases: [SettingsKey, unknown][] = [
    ['quality.default', 'ultra'],
    ['quality.default', 'AUTO'],
    ['session.mode.phone', 'mouse'],
    ['tailnet.mode', 'off'],
    ['session.hud', 'true'],
    ['session.hud', 1],
    ['lock.sensitiveHosts', ['h1', 1]],
    ['networks', [{ id: 'n', name: 'x', cidrs: [] }]],
    [
      'networks',
      [{ id: 'n', name: 'x', cidrs: [], vpnPools: [], extra: true }],
    ],
    ['networks', [{ id: 'n', name: 'x', cidrs: [1], vpnPools: [] }]],
    ['session.cursorSpeed', Number.NaN],
    ['session.cursorSpeed', Number.POSITIVE_INFINITY],
    ['session.cursorSpeed', '1.4'],
    ['hosts.prefs', { h1: { routePolicy: 'lan' } }],
    ['hosts.prefs', { h1: { tcpFallback: 'no' } }],
    ['hosts.prefs', { h1: { color: 'red' } }],
    ['hosts.prefs', { h1: null }],
    ['hosts.prefs', []],
    ['session.collapsedKeep', '1h'],
    ['session.collapsedKeep', null],
    ['session.collapsedKeep', 600000],
    ['keyboard.pasteSync', 'on'],
    ['keyboard.pasteSync', null],
  ];

  it.each(invalidCases)('rejects %s value %#', (key, value) => {
    expect(isSettingValue(key, value)).toBe(false);
  });

  const validCases: [SettingsKey, unknown][] = [
    [
      'networks',
      [
        {
          id: 'net-1',
          name: 'Office',
          cidrs: ['10.30.0.0/16'],
          vpnPools: ['10.99.0.0/16'],
        },
      ],
    ],
    ['hosts.prefs', { h1: {} }],
    ['hosts.prefs', { h1: { routePolicy: 'tailnet', lockOnEnd: true } }],
    ['session.cursorSpeed', 0.5],
    ['session.collapsedKeep', '10m'],
    ['session.collapsedKeep', '30m'],
    ['session.collapsedKeep', '2h'],
    ['session.collapsedKeep', 'always'],
  ];

  it.each(validCases)('accepts %s value %#', (key, value) => {
    expect(isSettingValue(key, value)).toBe(true);
  });
});

describe('settings domain guards', () => {
  it('accepts complete network entries and rejects extra fields', () => {
    const entry = {
      id: 'net-1',
      name: 'Office',
      cidrs: ['10.30.0.0/16'],
      vpnPools: ['10.99.0.0/16'],
    };
    expect(isNetworkEntry(entry)).toBe(true);
    expect(isNetworkEntry({ ...entry, extra: true })).toBe(false);
  });

  it('accepts partial host prefs and rejects unknown fields', () => {
    expect(isHostPrefsPatch({})).toBe(true);
    expect(isHostPrefsPatch({ routePolicy: 'intranet' })).toBe(true);
    expect(isHostPrefsPatch({ routePolicy: 'lan' })).toBe(false);
    expect(isHostPrefsPatch({ color: 'red' })).toBe(false);
    expect(isHostPrefsPatch([])).toBe(false);
  });

  it('recognizes only plain objects', () => {
    expect(isPlainObject({})).toBe(true);
    expect(isPlainObject(Object.create(null))).toBe(true);
    expect(isPlainObject([])).toBe(false);
    expect(isPlainObject(null)).toBe(false);
    expect(isPlainObject('value')).toBe(false);
  });
});
