import { useCallback, useEffect, useRef, useState } from 'react';
import { navigationRef } from '../../app/navigation/navigationRef';
import { getLaunchOptions } from '../../app/launchOptions';
import { harnessLog } from '../../native/devharness/log';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { useCoreEvent } from '../events/EventsProvider';
import { loadDirectory } from '../data/directoryQuery';
import { useLockState } from '../lock/lockState';
import { useSetting, useSettingsStore } from '../settings/store';
import { summarize } from './summary';
import type { SessionSample } from './summary';
import { parseAutoConnect, resolveHostId } from './target';

const APP_STARTED_AT_MS = Date.now();

function errorKind(error: unknown): string {
  return error instanceof RsCoreError ? error.kind : 'internal';
}

export default function AutoConnect(): React.JSX.Element | null {
  const [options] = useState(getLaunchOptions);
  if (options.autoConnect === undefined) {
    return null;
  }
  return <AutoConnectTarget target={options.autoConnect} />;
}

function AutoConnectTarget({ target }: { target: string }): null {
  const core = useCore();
  const settingsStore = useSettingsStore();
  const [onboardingDone] = useSetting('onboarding.done');
  const locked = useLockState(state => state.locked);
  const [navigationVersion, setNavigationVersion] = useState(0);
  const [options] = useState(getLaunchOptions);
  const raw = target;
  const attempted = useRef(false);
  const waitingLogged = useRef(false);
  const mounted = useRef(true);
  const targetHostId = useRef<string | null>(null);
  const sessionId = useRef<string | null>(null);
  const streaming = useRef(false);
  const streamingAtMs = useRef<number | null>(null);
  const stats = useRef<SessionSample[]>([]);
  const finished = useRef(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const moveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const finish = useCallback(
    (endedEarly: boolean) => {
      if (finished.current || !streaming.current) {
        return;
      }
      finished.current = true;
      if (closeTimer.current !== null) {
        clearTimeout(closeTimer.current);
        closeTimer.current = null;
      }
      if (moveTimer.current !== null) {
        clearTimeout(moveTimer.current);
        moveTimer.current = null;
      }

      const id = sessionId.current;
      if (id === null) {
        return;
      }
      const summary = summarize(stats.current);
      const durationS =
        streamingAtMs.current === null
          ? 0
          : Math.max(
              0,
              Math.round(((Date.now() - streamingAtMs.current) / 1000) * 1000) /
                1000,
            );
      if (!endedEarly) {
        try {
          core.sessions.close(id);
        } catch (error) {
          harnessLog('AUTOCONNECT', {
            event: 'error',
            error: errorKind(error),
          });
        }
      }
      harnessLog('AUTOCONNECT', {
        event: 'summary',
        sessionId: id,
        durationS,
        ...summary,
        ...(endedEarly ? { endedEarly: true } : {}),
      });
    },
    [core],
  );

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (closeTimer.current !== null) {
        clearTimeout(closeTimer.current);
      }
      if (moveTimer.current !== null) {
        clearTimeout(moveTimer.current);
      }
    };
  }, []);

  useEffect(() => {
    if (raw === undefined) {
      return;
    }
    const recheck = () => {
      setNavigationVersion(version => version + 1);
    };
    const unsubscribeState = navigationRef.addListener('state', recheck);
    const unsubscribeReady = navigationRef.addListener('ready', recheck);
    return () => {
      unsubscribeState();
      unsubscribeReady();
    };
  }, [raw]);

  useEffect(() => {
    if (raw === undefined) {
      return;
    }
    const timer = setTimeout(() => {
      if (attempted.current || waitingLogged.current) {
        return;
      }
      const reason = !settingsStore.get('onboarding.done')
        ? 'onboarding'
        : useLockState.getState().locked
        ? 'locked'
        : null;
      if (reason === null) {
        return;
      }
      waitingLogged.current = true;
      harnessLog('AUTOCONNECT', { event: 'waiting', reason });
    }, 3000);
    return () => clearTimeout(timer);
  }, [raw, settingsStore]);

  useEffect(() => {
    if (
      raw === undefined ||
      attempted.current ||
      !onboardingDone ||
      locked ||
      !navigationRef.isReady()
    ) {
      return;
    }
    // Re-read both stores: AppLockGate publishes its launch decision in an effect.
    if (
      !settingsStore.get('onboarding.done') ||
      useLockState.getState().locked
    ) {
      return;
    }
    const topRoute = navigationRef.getCurrentRoute()?.name;
    if (topRoute?.startsWith('Onboard') || topRoute === 'Session') {
      return;
    }

    attempted.current = true;
    const parsed = parseAutoConnect(raw);
    const run = async () => {
      try {
        if (parsed === null) {
          harnessLog('AUTOCONNECT', {
            event: 'error',
            error: 'invalidTarget',
          });
          return;
        }

        let resolved;
        if (parsed.kind === 'hostId') {
          resolved = { hostId: parsed.hostId };
        } else {
          const paired = await core.hosts.paired();
          resolved = resolveHostId(parsed, paired, { computers: [] });
          if ('error' in resolved) {
            const directory = await loadDirectory(core);
            resolved = resolveHostId(parsed, [], directory);
          }
        }
        if (!mounted.current) {
          return;
        }
        if ('error' in resolved) {
          harnessLog('AUTOCONNECT', {
            event: 'error',
            error: resolved.error,
          });
          return;
        }

        harnessLog('AUTOCONNECT', {
          event: 'resolve',
          kind: parsed.kind,
          hostId: resolved.hostId,
          ambiguous: 'ambiguous' in resolved,
        });
        targetHostId.current = resolved.hostId;
        navigationRef.navigate('Session', { hostId: resolved.hostId });
        harnessLog('AUTOCONNECT', {
          event: 'navigate',
          ms: Date.now() - APP_STARTED_AT_MS,
        });
      } catch (error) {
        if (mounted.current) {
          harnessLog('AUTOCONNECT', {
            event: 'error',
            error: errorKind(error),
          });
        }
      }
    };
    run().catch(error => {
      if (mounted.current) {
        harnessLog('AUTOCONNECT', {
          event: 'error',
          error: errorKind(error),
        });
      }
    });
  }, [core, locked, navigationVersion, onboardingDone, raw, settingsStore]);

  useCoreEvent('session.state', event => {
    if (event.hostId !== targetHostId.current) {
      return;
    }
    if (event.phase === 'streaming' && !streaming.current) {
      streaming.current = true;
      sessionId.current = event.sessionId;
      streamingAtMs.current = Date.now();
      harnessLog('AUTOCONNECT', {
        event: 'streaming',
        sessionId: event.sessionId,
        ms: Date.now() - APP_STARTED_AT_MS,
      });
      if (options.selfTestMove) {
        moveTimer.current = setTimeout(() => {
          harnessLog('AUTOCONNECT', {
            event: 'selfTestMove',
            status: 'unsupported',
          });
        }, 2000);
      }
      if (options.disconnectAfterS !== undefined) {
        closeTimer.current = setTimeout(() => {
          finish(false);
        }, options.disconnectAfterS * 1000);
      }
    } else if (
      event.phase === 'ended' &&
      event.sessionId === sessionId.current
    ) {
      finish(true);
    }
  });

  useCoreEvent('session.stats', event => {
    if (
      finished.current ||
      !streaming.current ||
      event.sessionId !== sessionId.current
    ) {
      return;
    }
    stats.current.push({
      rttMs: event.rttMs,
      fps: event.fps,
      decodeMs: event.decodeMs,
    });
  });

  return null;
}
