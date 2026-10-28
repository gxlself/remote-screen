import { isHostPrefsPatch, isSettingValue } from '../settings/domain';
import { DEFAULT_HOST_PREFS, settingsSchema } from '../settings/schema';

it('defaults to native sharing with per-host automatic selection', () => {
  expect(settingsSchema['connect.prefer'].def).toBe('native');
  expect(DEFAULT_HOST_PREFS.connectVia).toBe('auto');
});

it('accepts only the global connection preference values', () => {
  expect(isSettingValue('connect.prefer', 'native')).toBe(true);
  expect(isSettingValue('connect.prefer', 'host')).toBe(true);
  expect(isSettingValue('connect.prefer', 'auto')).toBe(false);
});

it.each(['auto', 'native', 'host'])(
  'accepts the per-host connection mode %s',
  connectVia => {
    expect(isHostPrefsPatch({ connectVia })).toBe(true);
    expect(isSettingValue('hosts.prefs', { h1: { connectVia } })).toBe(true);
  },
);

it('retains legacy preferences and rejects invalid new values', () => {
  expect(isHostPrefsPatch({ routePolicy: 'tailnet', tcpFallback: true })).toBe(
    true,
  );
  expect(isHostPrefsPatch({ connectVia: 'vnc' })).toBe(false);
  expect(isHostPrefsPatch({ connectVia: true })).toBe(false);
});
