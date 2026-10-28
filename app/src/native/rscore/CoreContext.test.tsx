import type { ReactNode } from 'react';
import { renderHook } from '@testing-library/react-native';
import { PlatformProvider, usePlatform } from '../platform/PlatformContext';
import { createRnPlatform } from '../platform/rnPlatform';
import { CoreProvider, useCore } from './CoreContext';
import type { RsCoreApi } from './RsCoreApi';

afterEach(() => {
  jest.restoreAllMocks();
});

it('provides the exact injected core object', async () => {
  // 此测试不调用服务；只验证上下文注入的对象。
  const core = { init: jest.fn() } as unknown as RsCoreApi;
  const { result } = await renderHook(() => useCore(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <CoreProvider core={core}>{children}</CoreProvider>
    ),
  });
  expect(result.current).toBe(core);
});

it('throws a clear error when CoreProvider is missing', async () => {
  jest.spyOn(console, 'error').mockImplementation(() => {});
  await expect(renderHook(() => useCore())).rejects.toThrow(
    'useCore 必须在 CoreProvider 里使用',
  );
});

it('provides the exact injected platform object', async () => {
  const platform = createRnPlatform();
  const { result } = await renderHook(() => usePlatform(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <PlatformProvider platform={platform}>{children}</PlatformProvider>
    ),
  });
  expect(result.current).toBe(platform);
});

it('throws a clear error when PlatformProvider is missing', async () => {
  jest.spyOn(console, 'error').mockImplementation(() => {});
  await expect(renderHook(() => usePlatform())).rejects.toThrow(
    'usePlatform 必须在 PlatformProvider 里使用',
  );
});
