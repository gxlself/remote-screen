import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { createRsCore } from '@app/native/rscore/createRsCore';

import { desktopBindings } from './bindings';
import { CoreProvider, useCore, useDesktop } from './CoreContext';
import { desktopApi } from './desktopApi';
import { fakeTransport } from './fake';

describe('CoreContext', () => {
  it('provides the injected core and desktop api', () => {
    const t = fakeTransport();
    const core = createRsCore(desktopBindings(t));
    const desktop = desktopApi(t);

    const { result } = renderHook(() => ({ core: useCore(), desktop: useDesktop() }), {
      wrapper: ({ children }: { children: ReactNode }) => (
        <CoreProvider core={core} desktop={desktop}>
          {children}
        </CoreProvider>
      ),
    });

    expect(result.current.core).toBe(core);
    expect(result.current.desktop).toBe(desktop);
  });

  it('throws outside the provider', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    expect(() => renderHook(() => useCore())).toThrow('必须在 CoreProvider 里使用');
    expect(() => renderHook(() => useDesktop())).toThrow('必须在 CoreProvider 里使用');
  });
});
