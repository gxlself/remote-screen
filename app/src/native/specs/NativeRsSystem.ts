import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type NativeSystemConstants = {
  appVersion: string;
  buildNumber: string;
  bundleId: string;
  modelId: string;
  maxFps: CodegenTypes.Int32;
};

export interface Spec extends TurboModule {
  getConstants(): NativeSystemConstants;
  copyText(text: string): void;
  hasText(): boolean;
  /** iOS UIPasteboard changeCount; -1 where the platform has none. */
  clipboardChangeCount(): number;
  readText(): Promise<string | null>;
  haptic(kind: string): void;
  notificationStatus(): Promise<string>;
  requestNotifications(): Promise<string>;
  requestLocalNetwork(timeoutMs: CodegenTypes.Double): Promise<string>;
}

export default TurboModuleRegistry.get<Spec>('RsSystem');
