import { useMemo, type JSX, type ReactNode } from 'react';

import { isTauri } from '../platform';

import { FakeIpc } from './fake';
import { IpcProvider } from './react';
import { createTauriTransport } from './tauri';
import type { Transport } from './transport';

declare global {
  interface Window {
    __fakeIpc?: FakeIpc;
  }
}

export function createDefaultTransport(): Transport {
  if (isTauri()) {
    return createTauriTransport();
  }

  const scenario = new URLSearchParams(location.search).get('scenario');
  const fake = FakeIpc.fromScenario(scenario);
  window.__fakeIpc = fake;
  return fake;
}

export function IpcRoot({ children }: { children: ReactNode }): JSX.Element {
  const transport = useMemo(() => createDefaultTransport(), []);
  return <IpcProvider transport={transport}>{children}</IpcProvider>;
}
