import type { SessionPhase } from '../../native/rscore/types';

export type ConnectStepKey = 'tunnel' | 'auth' | 'firstFrame';

export type ConnectStep = {
  key: ConnectStepKey;
  state: 'done' | 'active' | 'pending';
  seconds: number | null;
};

export type ConnectStepsInput = {
  phase: SessionPhase | null;
  startedAtMs: number;
  phaseAtMs: Partial<Record<SessionPhase, number>>;
  firstFrameAtMs: number | null;
  nowMs: number;
};

export type ConnectStepsResult = {
  steps: ConnectStep[];
  elapsedS: number;
  stuck: ConnectStepKey | null;
};

// 连接正常也要几秒；太早说「还在等」会让人以为出了问题。
const STUCK_AFTER_S = 8;
/** 进入推流后这么久还没有首帧，就给重试和降低画质。 */
export const FIRST_FRAME_SLOW_MS = 15_000;

function secondsBetween(fromMs: number, toMs: number): number {
  return Math.max(0, (toMs - fromMs) / 1000);
}

function roundedSeconds(seconds: number): number {
  return Math.round(seconds * 10) / 10;
}

function earliest(...values: Array<number | undefined>): number | undefined {
  const defined = values.filter(
    (value): value is number => value !== undefined,
  );
  return defined.length === 0 ? undefined : Math.min(...defined);
}

function doneStep(
  key: ConnectStepKey,
  startMs: number,
  endMs: number,
): ConnectStep {
  return {
    key,
    state: 'done',
    seconds: roundedSeconds(secondsBetween(startMs, endMs)),
  };
}

function activeStep(
  key: ConnectStepKey,
  startMs: number,
  nowMs: number,
): ConnectStep {
  return {
    key,
    state: 'active',
    seconds: roundedSeconds(secondsBetween(startMs, nowMs)),
  };
}

function pendingStep(key: ConnectStepKey): ConnectStep {
  return { key, state: 'pending', seconds: null };
}

/**
 * Reduces session phase timestamps into the three visible connection steps.
 *
 * 07 does not map phases to steps. Following 03 §4, TLS setup is the tunnel
 * step, the Host's WhoIs identity check is the auth step, and streaming starts
 * the wait for video. The visible copy avoids these protocol names.
 */
export function connectSteps(input: ConnectStepsInput): ConnectStepsResult {
  const { phase, startedAtMs, phaseAtMs, firstFrameAtMs, nowMs } = input;
  const tunnelEnd = earliest(
    phaseAtMs.authenticating,
    phaseAtMs.pendingApproval,
    phaseAtMs.streaming,
  );
  const authEnd = earliest(phaseAtMs.pendingApproval, phaseAtMs.streaming);

  const tunnel =
    tunnelEnd === undefined
      ? activeStep('tunnel', startedAtMs, nowMs)
      : doneStep('tunnel', startedAtMs, tunnelEnd);
  const auth =
    tunnel.state !== 'done'
      ? pendingStep('auth')
      : authEnd !== undefined
      ? doneStep('auth', tunnelEnd ?? startedAtMs, authEnd)
      : phase === 'authenticating'
      ? activeStep('auth', tunnelEnd ?? startedAtMs, nowMs)
      : pendingStep('auth');
  const firstFrame =
    auth.state !== 'done'
      ? pendingStep('firstFrame')
      : firstFrameAtMs !== null
      ? doneStep('firstFrame', authEnd ?? startedAtMs, firstFrameAtMs)
      : phase === 'streaming'
      ? activeStep('firstFrame', authEnd ?? startedAtMs, nowMs)
      : pendingStep('firstFrame');

  const steps = [tunnel, auth, firstFrame];
  const elapsedS = roundedSeconds(secondsBetween(startedAtMs, nowMs));
  const active = steps.find(item => item.state === 'active');
  return {
    steps,
    elapsedS,
    stuck: elapsedS >= STUCK_AFTER_S && active ? active.key : null,
  };
}
