import NativeSecureKey from '../specs/NativeSecureKey';
import type {
  DeviceKeyInfo,
  SigningBenchmark,
} from '../specs/NativeSecureKey';

export type { DeviceKeyInfo, SigningBenchmark } from '../specs/NativeSecureKey';

export function getDeviceKeyInfo(): Promise<DeviceKeyInfo | null> {
  if (NativeSecureKey == null) {
    return Promise.resolve(null);
  }
  return NativeSecureKey.info();
}

export function benchmarkSigning(count: number): Promise<SigningBenchmark | null> {
  if (NativeSecureKey == null) {
    return Promise.resolve(null);
  }
  return NativeSecureKey.benchmark(count);
}
