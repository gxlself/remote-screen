import { benchmarkSigning, getDeviceKeyInfo } from './secureKey';
import type { DeviceKeyInfo, SigningBenchmark } from '../specs/NativeSecureKey';

let mockNativeModule: {
  info: () => Promise<DeviceKeyInfo>;
  benchmark: (count: number) => Promise<SigningBenchmark>;
} | null = null;

jest.mock('../specs/NativeSecureKey', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  mockNativeModule = null;
});

it('returns null when the native module is unavailable', async () => {
  await expect(getDeviceKeyInfo()).resolves.toBeNull();
  await expect(benchmarkSigning(200)).resolves.toBeNull();
});

it('passes through native info and benchmark values', async () => {
  const info: DeviceKeyInfo = {
    registered: true,
    secureEnclave: false,
    fp: 'SHA256 00:01',
    deviceId: 'aaaqeayeaudaocaj',
    rotatedThisLaunch: true,
  };
  const benchmark: SigningBenchmark = {
    count: 200,
    p50Ms: 1.25,
    p95Ms: 2.5,
    maxMs: 3,
  };
  const infoMock = jest.fn<Promise<DeviceKeyInfo>, []>().mockResolvedValue(info);
  const benchmarkMock = jest.fn<Promise<SigningBenchmark>, [number]>().mockResolvedValue(benchmark);
  mockNativeModule = { info: infoMock, benchmark: benchmarkMock };

  await expect(getDeviceKeyInfo()).resolves.toBe(info);
  await expect(benchmarkSigning(-7)).resolves.toBe(benchmark);
  expect(infoMock).toHaveBeenCalledWith();
  expect(benchmarkMock).toHaveBeenCalledWith(-7);
});
