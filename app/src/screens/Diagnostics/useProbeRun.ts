import { useCallback, useEffect, useRef, useState } from 'react';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { EarlyProbeEvents } from '../../features/events/earlyProbeEvents';
import {
  applyProbeResult,
  expectedRows,
  finishRows,
  type ProbeRow,
} from '../../features/diag/probeRows';
import { useCore } from '../../native/rscore/CoreContext';
import type { Computer } from '../../native/rscore/types';

const PROBE_TIMEOUT_MS = 12_000;

export type ProbeRun = {
  rows: ProbeRow[];
  running: boolean;
  startedAt: number | null;
  finishedAtUs: number | null;
  start(): void;
  stop(): void;
};

type RunToken = {
  probeId: string | null;
  cancelled: boolean;
};

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message !== '') {
    return error.message;
  }
  return String(error);
}

export function useProbeRun(computer: Computer | null): ProbeRun {
  const core = useCore();
  const [rows, setRows] = useState<ProbeRow[]>([]);
  const [running, setRunning] = useState(false);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [finishedAtUs, setFinishedAtUs] = useState<number | null>(null);
  const tokenRef = useRef<RunToken | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const earlyEventsRef = useRef(new EarlyProbeEvents());
  const rowsRef = useRef<ProbeRow[]>([]);
  const computerRef = useRef<Computer | null>(computer);
  computerRef.current = computer;

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const finish = useCallback(
    (snapshot?: ProbeRow[]) => {
      clearTimer();
      earlyEventsRef.current.clear();
      setRunning(false);
      setFinishedAtUs(Date.now() * 1000);
      const out = finishRows(snapshot ?? rowsRef.current);
      rowsRef.current = out;
      setRows(out);
    },
    [clearTimer],
  );

  const stop = useCallback(() => {
    if (tokenRef.current !== null) {
      tokenRef.current.cancelled = true;
    }
    tokenRef.current = null;
    clearTimer();
    earlyEventsRef.current.clear();
    setRunning(false);
    setFinishedAtUs(Date.now() * 1000);
    const next = finishRows(rowsRef.current);
    rowsRef.current = next;
    setRows(next);
  }, [clearTimer]);

  const start = useCallback(() => {
    const target = computerRef.current;
    if (target === null) {
      return;
    }

    if (tokenRef.current !== null) {
      tokenRef.current.cancelled = true;
    }
    clearTimer();
    earlyEventsRef.current.clear();

    const token: RunToken = { probeId: null, cancelled: false };
    tokenRef.current = token;
    rowsRef.current = expectedRows(target);
    setRows(rowsRef.current);
    setRunning(true);
    setStartedAt(Date.now());
    setFinishedAtUs(null);

    const failAll = (error: unknown) => {
      if (token.cancelled || tokenRef.current !== token) {
        return;
      }
      tokenRef.current = null;
      clearTimer();
      earlyEventsRef.current.clear();
      setRunning(false);
      setFinishedAtUs(Date.now() * 1000);
      const reason = errorMessage(error);
      const next = rowsRef.current.map(item => ({
        ...item,
        state: 'fail' as const,
        outcome: 'timeout' as const,
        ms: null,
        reason,
      }));
      rowsRef.current = next;
      setRows(next);
    };

    timerRef.current = setTimeout(() => {
      if (token.cancelled || tokenRef.current !== token) {
        return;
      }
      tokenRef.current = null;
      earlyEventsRef.current.clear();
      finish();
    }, PROBE_TIMEOUT_MS);

    let pending: Promise<string>;
    try {
      pending = core.diag.probe(target.hostId);
    } catch (error) {
      failAll(error);
      return;
    }
    pending.then(probeId => {
      if (token.cancelled || tokenRef.current !== token) {
        return;
      }
      token.probeId = probeId;
      for (const event of earlyEventsRef.current.take(probeId)) {
        if (token.cancelled || tokenRef.current !== token) {
          break;
        }
        const next = applyProbeResult(rowsRef.current, event);
        rowsRef.current = next;
        setRows(next);
        if (event.last) {
          tokenRef.current = null;
          finish(next);
          break;
        }
      }
    }, failAll);
  }, [clearTimer, core, finish]);

  useCoreEvent('probe.result', event => {
    const token = tokenRef.current;
    if (token === null || token.cancelled) {
      return;
    }
    if (token.probeId === null) {
      earlyEventsRef.current.push(event);
      return;
    }
    if (event.probeId !== token.probeId) {
      return;
    }

    const next = applyProbeResult(rowsRef.current, event);
    rowsRef.current = next;
    setRows(next);
    if (event.last) {
      tokenRef.current = null;
      finish(next);
    }
  });

  const hostId = computer?.hostId ?? null;
  useEffect(() => {
    const earlyEvents = earlyEventsRef.current;
    if (hostId !== null) {
      start();
    } else {
      stop();
      rowsRef.current = [];
      setRows([]);
      setStartedAt(null);
      setFinishedAtUs(null);
    }

    return () => {
      if (tokenRef.current !== null) {
        tokenRef.current.cancelled = true;
      }
      tokenRef.current = null;
      clearTimer();
      earlyEvents.clear();
    };
  }, [clearTimer, hostId, start, stop]);

  return {
    rows,
    running,
    startedAt,
    finishedAtUs,
    start,
    stop,
  };
}
