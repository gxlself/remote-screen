import { useEffect, useState } from 'react';
import { AppState, Platform } from 'react-native';
import {
  androidBiometryAuthNames,
  devicePasscodeAuthName,
} from '../../i18n/zh/appLock';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { AuthCapability } from '../../native/platform/PlatformServices';

export function authName(cap: AuthCapability): string | null {
  switch (cap.biometry) {
    case 'faceId':
      return 'Face ID';
    case 'touchId':
      return 'Touch ID';
    case 'opticId':
      return 'Optic ID';
    case 'fingerprint':
      return androidBiometryAuthNames.fingerprint;
    case 'face':
      return androidBiometryAuthNames.face;
    case 'iris':
      return androidBiometryAuthNames.iris;
    case 'biometric':
      return androidBiometryAuthNames.biometric;
    case 'none':
      return cap.passcode ? devicePasscodeAuthName : null;
  }
}

export function fallbackAuthName(): string {
  return Platform.OS === 'android'
    ? androidBiometryAuthNames.biometric
    : 'Face ID';
}

export function authIcon(cap: AuthCapability): 'faceId' | 'lock' {
  return cap.biometry === 'faceId' ? 'faceId' : 'lock';
}

export function useAuthCapability(): AuthCapability {
  const platform = usePlatform();
  const [capability, setCapability] = useState(() => platform.authCapability());

  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => {
      if (state === 'active') {
        setCapability(platform.authCapability());
      }
    });
    return () => subscription.remove();
  }, [platform]);

  return capability;
}

export function useAuthName(): string {
  return authName(useAuthCapability()) ?? fallbackAuthName();
}
