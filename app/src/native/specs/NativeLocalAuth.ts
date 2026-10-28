import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  authenticate(reason: string, fallbackTitle: string): Promise<string>;
  getBiometryType(): string;
  canAuthenticate(): { biometrics: boolean; passcode: boolean };
  setPrivacyCover(enabled: boolean): void;
}

export default TurboModuleRegistry.get<Spec>('LocalAuth');
