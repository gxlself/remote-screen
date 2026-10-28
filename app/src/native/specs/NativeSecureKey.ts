import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type DeviceKeyInfo = {
  registered: boolean;
  secureEnclave: boolean;
  fp: string;
  deviceId: string;
  rotatedThisLaunch: boolean;
};

export type SigningBenchmark = {
  count: number;
  p50Ms: number;
  p95Ms: number;
  maxMs: number;
};

export interface Spec extends TurboModule {
  info(): Promise<DeviceKeyInfo>;
  benchmark(count: number): Promise<SigningBenchmark>;
}

export default TurboModuleRegistry.get<Spec>('SecureKey');
