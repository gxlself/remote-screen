import { useCallback, useEffect, useRef, useState } from 'react';
import type { AdapterErrorInfo } from '../../native/rscore/types';
import {
  subscribeWindowPhase,
  useWindowId,
  windowPhaseNow,
} from '../session/WindowContext';
import {
  isNetworkDrop,
  LOST_IN_BACKGROUND,
  RECONNECT_DELAYS_MS,
  type AdapterUiPhase,
} from './logic';

export type AdapterReconnect = {
  /** Reconnecting by itself; the page keeps the last picture and an overlay. */
  active: boolean;
  /** 1-based attempt shown while active. */
  attempt: number;
  max: number;
  /** Stops at once; the page then shows the error that started it. */
  cancel(): void;
};

const IDLE = { active: false, attempt: 0 };

type Session = {
  phase: AdapterUiPhase;
  error: AdapterErrorInfo | null;
  retry(): void;
  end(error: AdapterErrorInfo): void;
  isAlive(): boolean | null;
};

/**
 * Reconnects a session that was up and then dropped (timeout or a broken
 * link), with backoff, up to RECONNECT_DELAYS_MS.length times. Waits for the
 * foreground before trying, and on returning checks that the native session
 * survived. Before the first picture, errors go straight to the error page.
 * The foreground is this page's window's.
 */
export function useAdapterReconnect(session: Session): AdapterReconnect {
  const windowId = useWindowId();
  const latest = useRef(session);
  latest.current = session;
  const streamed = useRef(false);
  const attempts = useRef(0);
  const cancelled = useRef(false);
  const cause = useRef<AdapterErrorInfo | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const waitingForeground = useRef(false);
  const [view, setView] = useState(IDLE);

  const clearTimer = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const schedule = useCallback(() => {
    clearTimer();
    if (windowPhaseNow(windowId) === 'background') {
      waitingForeground.current = true;
      return;
    }
    waitingForeground.current = false;
    const delay = RECONNECT_DELAYS_MS[attempts.current] ?? 0;
    timer.current = setTimeout(() => {
      timer.current = null;
      attempts.current += 1;
      latest.current.retry();
    }, delay);
  }, [clearTimer, windowId]);

  const { phase, error } = session;
  useEffect(() => {
    if (phase === 'streaming') {
      streamed.current = true;
      attempts.current = 0;
      cancelled.current = false;
      cause.current = null;
      setView(IDLE);
      return;
    }
    if (phase !== 'ended' || !streamed.current || cancelled.current) {
      return;
    }
    if (timer.current !== null || waitingForeground.current) {
      return;
    }
    if (
      error === null ||
      !isNetworkDrop(error.code) ||
      attempts.current >= RECONNECT_DELAYS_MS.length
    ) {
      setView(IDLE);
      return;
    }
    cause.current ??= error;
    setView({ active: true, attempt: attempts.current + 1 });
    schedule();
  }, [error, phase, schedule]);

  useEffect(() => {
    return subscribeWindowPhase(windowId, state => {
      const current = latest.current;
      if (state === 'background') {
        if (timer.current !== null) {
          clearTimer();
          waitingForeground.current = true;
        }
        return;
      }
      if (state !== 'active') {
        return;
      }
      if (waitingForeground.current) {
        schedule();
        return;
      }
      // Sockets often die while the app sleeps without an event reaching us.
      if (current.phase === 'streaming' && current.isAlive() === false) {
        current.end(LOST_IN_BACKGROUND);
      }
    });
  }, [clearTimer, schedule, windowId]);

  useEffect(() => clearTimer, [clearTimer]);

  const cancel = useCallback(() => {
    clearTimer();
    waitingForeground.current = false;
    cancelled.current = true;
    setView(IDLE);
    const current = latest.current;
    if (current.phase !== 'ended') {
      current.end(cause.current ?? LOST_IN_BACKGROUND);
    }
  }, [clearTimer]);

  // The render that delivers the drop already counts as reconnecting, so the
  // error page never flashes before the effect above schedules a retry.
  const starting =
    !view.active &&
    phase === 'ended' &&
    streamed.current &&
    !cancelled.current &&
    timer.current === null &&
    !waitingForeground.current &&
    error !== null &&
    isNetworkDrop(error.code) &&
    attempts.current < RECONNECT_DELAYS_MS.length;

  return {
    active: view.active || starting,
    attempt: starting ? attempts.current + 1 : view.attempt,
    max: RECONNECT_DELAYS_MS.length,
    cancel,
  };
}
