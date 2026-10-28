import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { getLaunchOptions } from '../../app/launchOptions';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { ServicesProviders } from './ServicesProviders';
import { createAppServices, defaultDeps } from './appServices';
import type { AppServices } from './appServices';
import { bootServices, selectCoreMode } from './coreBoot';
import BootScreen from './BootScreen';
import { setupAppFocus } from './queryClient';

type BootState =
  | { kind: 'booting' }
  | { kind: 'ready'; services: AppServices }
  | { kind: 'failed'; message: string };

function createBootSession() {
  const opts = getLaunchOptions();
  const mode = selectCoreMode(opts, defaultDeps.os);
  if (mode.kind === 'fake') {
    // Release builds never fall back to demo data on a phone: without a data
    // directory the app cannot work, so say so instead.
    if (mode.reason === 'noDataDir' && !__DEV__) {
      return { kind: 'unavailable' } as const;
    }
    return { kind: 'fake', services: createAppServices() } as const;
  }
  // Keep a loaded Core across retries; a failed load can itself be retried.
  let core: RsCoreApi | undefined;
  return {
    kind: 'real',
    opts,
    deps: {
      ...defaultDeps,
      loadReal: () => {
        core ??= defaultDeps.loadReal();
        return core;
      },
    },
  } as const;
}

const NO_DATA_DIR_MESSAGE = 'io: data directory unavailable';

function failureMessage(error: unknown): string {
  const kind =
    error !== null &&
    typeof error === 'object' &&
    'kind' in error &&
    typeof error.kind === 'string'
      ? error.kind
      : 'internal';
  const message = error instanceof Error ? error.message : String(error);
  return `${kind}: ${message.slice(0, 200)}`;
}

export default function DataProviders({ children }: { children: ReactNode }) {
  const [session] = useState(createBootSession);
  const [state, setState] = useState<BootState>(() => {
    if (session.kind === 'fake') {
      return { kind: 'ready', services: session.services };
    }
    if (session.kind === 'unavailable') {
      return { kind: 'failed', message: NO_DATA_DIR_MESSAGE };
    }
    return { kind: 'booting' };
  });
  const pending = useRef<Promise<AppServices> | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => setupAppFocus(), []);
  useEffect(() => {
    if (session.kind !== 'real') {
      return;
    }
    // StrictMode replays this effect: subscribe again to the same boot attempt.
    pending.current ??= bootServices(session.opts, session.deps);
    let active = true;
    pending.current.then(
      services => {
        if (active) {
          setState({ kind: 'ready', services });
        }
      },
      error => {
        if (active) {
          setState({ kind: 'failed', message: failureMessage(error) });
        }
      },
    );
    return () => {
      active = false;
    };
  }, [session, attempt]);

  if (state.kind === 'ready') {
    return (
      <ServicesProviders {...state.services}>{children}</ServicesProviders>
    );
  }
  return (
    <BootScreen
      state={state.kind}
      message={state.kind === 'failed' ? state.message : undefined}
      onRetry={
        session.kind === 'real'
          ? () => {
              pending.current = null;
              setState({ kind: 'booting' });
              setAttempt(value => value + 1);
            }
          : undefined
      }
    />
  );
}
