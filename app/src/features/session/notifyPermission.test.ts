import type { PlatformServices } from '../../native/platform/PlatformServices';
import type { DevicePlatform } from '../../native/rscore/types';
import { MemoryKv } from '../storage/kv';
import {
  NOTIFY_ASKED_KEY,
  ensureNotifyBeforeSession,
  needsNotifyPrompt,
} from './notifyPermission';

function platformFor(
  platform: DevicePlatform,
  osVersion: string,
  requestNotifications = jest.fn<Promise<'granted' | 'denied'>, []>(),
): Pick<PlatformServices, 'deviceInfo' | 'requestNotifications'> {
  return {
    deviceInfo: () => ({
      platform,
      isPad: platform === 'ipados',
      model: 'Test device',
      osVersion,
      appVersion: '1.0.0',
    }),
    requestNotifications,
  };
}

it.each([
  ['android', '33', false, true],
  ['android', '34', false, true],
  ['android', '32', false, false],
  ['android', '33', true, false],
  ['android', 'abc', false, false],
  ['ios', '18', false, false],
  ['ipados', '18', false, false],
] satisfies [DevicePlatform, string, boolean, boolean][])(
  'needsNotifyPrompt(%s, %s, asked=%s) is %s',
  (platform, osVersion, asked, expected) => {
    expect(needsNotifyPrompt({ platform, osVersion }, asked)).toBe(expected);
  },
);

it('asks once on the first Android 13+ session and records the attempt first', async () => {
  const kv = new MemoryKv();
  const requestNotifications = jest
    .fn<Promise<'granted' | 'denied'>, []>()
    .mockImplementation(async () => {
      expect(kv.getString(NOTIFY_ASKED_KEY)).toBe('1');
      return 'granted';
    });
  const platform = platformFor('android', '33', requestNotifications);

  await expect(ensureNotifyBeforeSession({ platform, kv })).resolves.toBe(
    'granted',
  );
  expect(requestNotifications).toHaveBeenCalledTimes(1);
  expect(kv.getString(NOTIFY_ASKED_KEY)).toBe('1');

  await expect(ensureNotifyBeforeSession({ platform, kv })).resolves.toBe(
    'skipped',
  );
  expect(requestNotifications).toHaveBeenCalledTimes(1);
});

it('returns denied and keeps the marker when the permission request throws', async () => {
  const kv = new MemoryKv();
  const requestNotifications = jest
    .fn<Promise<'granted' | 'denied'>, []>()
    .mockRejectedValue(new Error('permission unavailable'));

  await expect(
    ensureNotifyBeforeSession({
      platform: platformFor('android', '33', requestNotifications),
      kv,
    }),
  ).resolves.toBe('denied');
  expect(kv.getString(NOTIFY_ASKED_KEY)).toBe('1');
});

it('skips iOS without requesting or writing the marker', async () => {
  const kv = new MemoryKv();
  const requestNotifications = jest.fn<Promise<'granted' | 'denied'>, []>();

  await expect(
    ensureNotifyBeforeSession({
      platform: platformFor('ios', '18', requestNotifications),
      kv,
    }),
  ).resolves.toBe('skipped');
  expect(requestNotifications).not.toHaveBeenCalled();
  expect(kv.keys()).toEqual([]);
});
