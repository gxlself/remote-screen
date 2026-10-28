import { useCallback, useEffect, useRef, useState } from 'react';

import { toRsCoreError } from '@app/native/rscore/errors';
import type { RsCoreApi } from '@app/native/rscore/RsCoreApi';
import type { DevicePlatform } from '@app/native/rscore/types';

import type { DesktopApi, DesktopInfo } from '../core/desktopApi';

import { redactMessage } from './redact';

export type StartupState =
  | { phase: 'starting' }
  | { phase: 'registering' }
  | { phase: 'ready' }
  /** `message` is already redacted. */
  | { phase: 'failed'; message: string };

export type Startup = {
  state: StartupState;
  retry: () => void;
};

function initConfig(info: DesktopInfo): Parameters<RsCoreApi['init']>[0] {
  return {
    dataDir: info.dataDir,
    appVersion: info.version,
    // CoreConfig.platform lists phone platforms only (C6 keeps it). The shell builds the
    // desktop Core from its own config, so coreInit is a no-op and the value is informational.
    platform: info.platform as string as DevicePlatform,
  };
}

/** desktop.info → core.init → identity.ensure. */
export function useStartup(core: RsCoreApi, desktop: DesktopApi): Startup {
  const [state, setState] = useState<StartupState>({ phase: 'starting' });
  const info = useRef<DesktopInfo | null>(null);
  const started = useRef(false);
  const running = useRef(false);
  const mounted = useRef(false);

  // Every setState here comes after an await; retry sets the visible phase itself.
  const run = useCallback(async () => {
    running.current = true;
    try {
      let current = info.current;
      if (current === null) {
        current = await desktop.info();
        await core.init(initConfig(current));
        info.current = current;
        if (mounted.current) {
          setState({ phase: 'registering' });
        }
      }
      await core.identity.ensure(current.deviceName);
      if (mounted.current) {
        setState({ phase: 'ready' });
      }
    } catch (e) {
      if (mounted.current) {
        const message = toRsCoreError(e).message;
        setState({
          phase: 'failed',
          message: redactMessage(message, info.current?.deviceName ?? ''),
        });
      }
    } finally {
      running.current = false;
    }
  }, [core, desktop]);

  useEffect(() => {
    mounted.current = true;
    if (!started.current) {
      started.current = true;
      void run();
    }
    return () => {
      mounted.current = false;
    };
  }, [run]);

  const retry = useCallback(() => {
    if (running.current) {
      return;
    }
    setState({ phase: info.current === null ? 'starting' : 'registering' });
    void run();
  }, [run]);

  return { state, retry };
}
