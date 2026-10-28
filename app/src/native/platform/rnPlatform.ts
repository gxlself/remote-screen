import { Linking, Platform, Share } from 'react-native';
import type { ShareContent } from 'react-native';
import type { AuthCapability, PlatformServices } from './PlatformServices';
import { authenticate } from './impl/auth';
import { biometryType, canAuthenticate } from '../localAuth/localAuth';
import {
  appVersion,
  copyText,
  haptic,
  readText,
  requestLocalNetwork,
  requestNotifications,
} from './impl/system';

function authCapability(): AuthCapability {
  const { biometrics, passcode } = canAuthenticate();
  if (!biometrics) {
    return { biometry: 'none', passcode };
  }
  switch (biometryType()) {
    case 'faceID':
      return { biometry: 'faceId', passcode };
    case 'touchID':
      return { biometry: 'touchId', passcode };
    case 'opticID':
      return { biometry: 'opticId', passcode };
    case 'fingerprint':
      return { biometry: 'fingerprint', passcode };
    case 'face':
      return { biometry: 'face', passcode };
    case 'iris':
      return { biometry: 'iris', passcode };
    case 'biometric':
      return { biometry: 'biometric', passcode };
    default:
      return { biometry: 'none', passcode };
  }
}

// Implementations are split by owning card: auth.ts is P9; system.ts is P12.
// URL/share and device discovery remain in this RN adapter until their cards
// replace them with native implementations.
export function createRnPlatform(): PlatformServices {
  return {
    authenticate,
    authCapability,
    copyText,
    readText,
    async openUrl(url) {
      try {
        await Linking.openURL(url);
        return true;
      } catch {
        return false;
      }
    },
    async canOpenUrl(url) {
      try {
        return await Linking.canOpenURL(url);
      } catch {
        return false;
      }
    },
    async share(opts) {
      const content: ShareContent =
        opts.url !== undefined
          ? {
              url: opts.url,
              ...(opts.text === undefined ? {} : { message: opts.text }),
              ...(opts.title === undefined ? {} : { title: opts.title }),
            }
          : {
              message: opts.text ?? '',
              ...(opts.title === undefined ? {} : { title: opts.title }),
            };
      await Share.share(content);
    },
    haptic,
    requestNotifications,
    requestLocalNetwork,
    deviceInfo() {
      const isPad = Platform.OS === 'ios' && Platform.isPad;
      return {
        platform:
          Platform.OS === 'ios' ? (isPad ? 'ipados' : 'ios') : 'android',
        isPad,
        model:
          Platform.OS === 'android'
            ? Platform.constants.Model
            : isPad
            ? 'iPad'
            : 'iPhone',
        osVersion: String(Platform.Version),
        appVersion: appVersion(),
      };
    },
  };
}
