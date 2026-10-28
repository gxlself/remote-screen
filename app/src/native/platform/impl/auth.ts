import type { AuthResult } from '../PlatformServices';
import {
  authenticate as authenticateLocalAuth,
  biometryType as getBiometryType,
} from '../../localAuth/localAuth';

export async function authenticate(reason: string): Promise<AuthResult> {
  return authenticateLocalAuth(reason);
}

export function biometryType(): string {
  return getBiometryType();
}
