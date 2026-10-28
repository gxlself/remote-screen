import { Platform } from 'react-native';
import { FakePlatform } from '../../native/platform/fakePlatform';
import {
  ANDROID_APP_ID,
  IOS_APP_STORE_ID,
  appUpdateUrls,
  currentOs,
  openFirst,
  playStoreUrls,
  tailscaleUrls,
} from './externalApps';

afterEach(() => {
  jest.restoreAllMocks();
});

describe('external app URLs', () => {
  it('builds the Play Store links for a package', () => {
    expect(playStoreUrls('com.example.app')).toEqual([
      'market://details?id=com.example.app',
      'https://play.google.com/store/apps/details?id=com.example.app',
    ]);
  });

  it('keeps the iOS Tailscale links unchanged', () => {
    expect(tailscaleUrls('ios')).toEqual([
      'tailscale://',
      'https://apps.apple.com/app/tailscale/id1470499037',
    ]);
  });

  it('falls back to the Tailscale Play Store page on Android', () => {
    expect(tailscaleUrls('android')).toEqual([
      'tailscale://',
      'market://details?id=com.tailscale.ipn',
      'https://play.google.com/store/apps/details?id=com.tailscale.ipn',
    ]);
  });

  it('opens this app in the App Store on iOS', () => {
    expect(IOS_APP_STORE_ID).toBe('6826963518');
    expect(appUpdateUrls('ios')).toEqual([
      'itms-apps://apps.apple.com/app/id6826963518',
      'https://apps.apple.com/app/id6826963518',
    ]);
  });

  it('opens this app in Google Play on Android', () => {
    expect(ANDROID_APP_ID).toBe('ai.gxlself.remotescreen');
    expect(appUpdateUrls('android')).toEqual([
      'market://details?id=ai.gxlself.remotescreen',
      'https://play.google.com/store/apps/details?id=ai.gxlself.remotescreen',
    ]);
  });

  it('reads the current OS when each function is called', () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    expect(currentOs()).toBe('ios');
    expect(tailscaleUrls()).toEqual([
      'tailscale://',
      'https://apps.apple.com/app/tailscale/id1470499037',
    ]);

    jest.replaceProperty(Platform, 'OS', 'android');
    expect(currentOs()).toBe('android');
    expect(appUpdateUrls()).toEqual([
      'market://details?id=ai.gxlself.remotescreen',
      'https://play.google.com/store/apps/details?id=ai.gxlself.remotescreen',
    ]);
  });
});

describe('openFirst', () => {
  it('stops after the first URL opens', async () => {
    const platform = new FakePlatform();
    jest.spyOn(platform, 'openUrl');

    await expect(openFirst(platform, ['first://', 'second://'])).resolves.toBe(
      'first://',
    );
    expect(platform.opened).toEqual(['first://']);
    expect(platform.openUrl).toHaveBeenCalledTimes(1);
  });

  it('tries the next URL when the first returns false', async () => {
    const platform = new FakePlatform();
    jest.spyOn(platform, 'openUrl').mockResolvedValueOnce(false);

    await expect(openFirst(platform, ['first://', 'second://'])).resolves.toBe(
      'second://',
    );
    expect(platform.opened).toEqual(['second://']);
    expect(platform.openUrl).toHaveBeenNthCalledWith(1, 'first://');
    expect(platform.openUrl).toHaveBeenNthCalledWith(2, 'second://');
    expect(platform.openUrl).toHaveBeenCalledTimes(2);
  });

  it('tries the next URL when the first rejects', async () => {
    const platform = new FakePlatform();
    jest
      .spyOn(platform, 'openUrl')
      .mockRejectedValueOnce(new Error('no handler'));

    await expect(openFirst(platform, ['first://', 'second://'])).resolves.toBe(
      'second://',
    );
    expect(platform.opened).toEqual(['second://']);
    expect(platform.openUrl).toHaveBeenCalledTimes(2);
  });

  it('returns null when every URL fails', async () => {
    const platform = new FakePlatform();
    platform.openUrlResult = false;
    jest.spyOn(platform, 'openUrl');

    await expect(openFirst(platform, ['first://', 'second://'])).resolves.toBe(
      null,
    );
    expect(platform.openUrl).toHaveBeenCalledTimes(2);
  });
});
