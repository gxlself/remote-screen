import { Platform } from 'react-native';
import type { PlatformServices } from '../../native/platform/PlatformServices';

export type AppOs = 'ios' | 'android';

export const ANDROID_APP_ID = 'ai.gxlself.remotescreen';
/** App Store Connect Apple ID of 远屏 Remote Screen. */
export const IOS_APP_STORE_ID = '6826963518';

export function currentOs(): AppOs {
  return Platform.OS === 'android' ? 'android' : 'ios';
}

export function playStoreUrls(packageId: string): readonly string[] {
  return [
    `market://details?id=${packageId}`,
    `https://play.google.com/store/apps/details?id=${packageId}`,
  ];
}

export function tailscaleUrls(os: AppOs = currentOs()): readonly string[] {
  return os === 'android'
    ? ['tailscale://', ...playStoreUrls('com.tailscale.ipn')]
    : ['tailscale://', 'https://apps.apple.com/app/tailscale/id1470499037'];
}

export function appUpdateUrls(os: AppOs = currentOs()): readonly string[] {
  return os === 'android'
    ? playStoreUrls(ANDROID_APP_ID)
    : [
        `itms-apps://apps.apple.com/app/id${IOS_APP_STORE_ID}`,
        `https://apps.apple.com/app/id${IOS_APP_STORE_ID}`,
      ];
}

export async function openFirst(
  platform: Pick<PlatformServices, 'openUrl'>,
  urls: readonly string[],
): Promise<string | null> {
  for (const url of urls) {
    try {
      if (await platform.openUrl(url)) {
        return url;
      }
    } catch {
      // Try the next URL when the platform rejects this one.
    }
  }
  return null;
}
