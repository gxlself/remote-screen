import type { PlatformServices } from '../../native/platform/PlatformServices';
import type { DevicePlatform } from '../../native/rscore/types';
import type { KvStore } from '../storage/kv';

export const NOTIFY_ASKED_KEY = 'perm.notifyAskedBeforeSession';

export function needsNotifyPrompt(
  info: { platform: DevicePlatform; osVersion: string },
  asked: boolean,
): boolean {
  const osVersion = Number(info.osVersion);
  return (
    info.platform === 'android' &&
    Number.isInteger(osVersion) &&
    osVersion >= 33 &&
    !asked
  );
}

export async function ensureNotifyBeforeSession(deps: {
  platform: Pick<PlatformServices, 'deviceInfo' | 'requestNotifications'>;
  kv: KvStore;
}): Promise<'granted' | 'denied' | 'skipped'> {
  const asked = deps.kv.getString(NOTIFY_ASKED_KEY) === '1';
  if (!needsNotifyPrompt(deps.platform.deviceInfo(), asked)) {
    return 'skipped';
  }

  // Mark first so a crash during the system prompt does not ask on every open.
  deps.kv.set(NOTIFY_ASKED_KEY, '1');
  try {
    return await deps.platform.requestNotifications();
  } catch {
    return 'denied';
  }
}
