import type { AuthResult } from '../platform/PlatformServices';
import NativeLocalAuth from '../specs/NativeLocalAuth';

const AUTH_RESULTS: ReadonlySet<string> = new Set([
  'ok',
  'cancelled',
  'unavailable',
  'failed',
]);

function isAuthResult(value: string): value is AuthResult {
  return AUTH_RESULTS.has(value);
}

export async function authenticate(reason: string): Promise<AuthResult> {
  const module = NativeLocalAuth;
  if (module == null) {
    return 'unavailable';
  }
  try {
    const result = await module.authenticate(reason, '用设备密码');
    return isAuthResult(result) ? result : 'failed';
  } catch {
    return 'failed';
  }
}

export function biometryType(): string {
  return NativeLocalAuth?.getBiometryType() ?? 'none';
}

export function canAuthenticate(): {
  biometrics: boolean;
  passcode: boolean;
} {
  return (
    NativeLocalAuth?.canAuthenticate() ?? {
      biometrics: false,
      passcode: false,
    }
  );
}

export function setPrivacyCover(enabled: boolean): void {
  NativeLocalAuth?.setPrivacyCover(enabled);
}
