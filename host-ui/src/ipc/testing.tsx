/**
 * Minimal page test:
 *
 * const { user, ipc } = renderPage(<Page />);
 * await flush();
 * expect(screen.getByText('在线')).toBeInTheDocument();
 * await user.click(screen.getByRole('button', { name: '断开' }));
 * await flush();
 * expect(ipc.calls).toContainEqual({
 *   method: 'sessions.disconnect',
 *   params: { session_id: 's-1' },
 * });
 */
import { act, cleanup, render, type RenderResult } from '@testing-library/react';
import userEvent, { type UserEvent } from '@testing-library/user-event';
import type { ReactElement } from 'react';
import { vi } from 'vitest';

import { FakePlatform, PlatformProvider } from '../platform';

import { FakeIpc } from './fake';
import { IpcProvider } from './react';

export type PageRenderResult = RenderResult & {
  ipc: FakeIpc;
  platform: FakePlatform;
  user: UserEvent;
};

interface FakeTimerGlobals {
  jest?: {
    advanceTimersByTime: (ms: number) => void;
  };
}

let fakeTimerGlobalsInstalled = false;

function installFakeTimerGlobals(): void {
  if (!vi.isFakeTimers() || fakeTimerGlobalsInstalled) {
    return;
  }
  (globalThis as FakeTimerGlobals).jest = {
    advanceTimersByTime: (ms) => vi.advanceTimersByTime(ms),
  };
  cleanup();
  fakeTimerGlobalsInstalled = true;
}

export function renderPage(
  ui: ReactElement,
  options: { ipc?: FakeIpc; platform?: FakePlatform; hash?: string } = {},
): PageRenderResult {
  const ipc = options.ipc ?? FakeIpc.fromScenario('controlled');
  const platform = options.platform ?? new FakePlatform();
  installFakeTimerGlobals();

  if (options.hash !== undefined) {
    location.hash = options.hash;
  }

  const user = userEvent.setup({
    advanceTimers: async (ms) => {
      if (vi.isFakeTimers()) {
        await vi.advanceTimersByTimeAsync(ms);
      }
    },
  });
  const result = render(
    <PlatformProvider platform={platform}>
      <IpcProvider transport={ipc}>{ui}</IpcProvider>
    </PlatformProvider>,
  );

  return { ...result, ipc, platform, user };
}

export async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
  await act(async () => {
    await Promise.resolve();
  });
}
