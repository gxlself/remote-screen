import type {
  CoreEventOf,
  NetPath,
  PairOutcome,
  PairStep,
} from '../../native/rscore/types';

export type PairUiState = {
  phase: 'starting' | 'progress' | 'success' | 'failed';
  step: PairStep | null;
  path: NetPath | null;
  latencyMs: number | null;
  fp: string | null;
  timeoutS: number;
  outcome: PairOutcome | null;
};

export const initialPairState: PairUiState = {
  phase: 'starting',
  step: null,
  path: null,
  latencyMs: null,
  fp: null,
  timeoutS: 30,
  outcome: null,
};

export type PairStepModel = {
  key: 'connect' | 'verify' | 'approve';
  state: 'done' | 'active' | 'pending' | 'failed';
};

type PairEvent =
  | CoreEventOf<'pairing.progress'>
  | CoreEventOf<'pairing.done'>
  | { type: 'local.outcome'; outcome: PairOutcome };

type StepStates = [
  PairStepModel['state'],
  PairStepModel['state'],
  PairStepModel['state'],
];

export function pairReducer(s: PairUiState, e: PairEvent): PairUiState {
  if (s.phase === 'success' || s.phase === 'failed') {
    return s;
  }

  switch (e.type) {
    case 'pairing.progress':
      return {
        ...s,
        phase: 'progress',
        step: e.step,
        path: e.path ?? s.path,
        latencyMs: e.latencyMs ?? s.latencyMs,
        fp: e.fp ?? s.fp,
        timeoutS: e.timeoutS ?? s.timeoutS,
      };
    case 'pairing.done':
    case 'local.outcome':
      return {
        ...s,
        phase: e.outcome.status === 'ok' ? 'success' : 'failed',
        outcome: e.outcome,
      };
  }
}

export function pairSteps(s: PairUiState): PairStepModel[] {
  if (s.phase === 'success') {
    return toModels(['done', 'done', 'done']);
  }

  const base = baseSteps(s.step);
  if (s.phase !== 'failed' || s.outcome === null) {
    return toModels(base);
  }

  switch (s.outcome.status) {
    case 'mitm':
    case 'badCode':
    case 'expired':
    case 'locked':
    case 'methodDisabled':
      return toModels(['done', 'failed', 'pending']);
    case 'denied':
    case 'timeout':
      return toModels(['done', 'done', 'failed']);
    case 'disabledOnPath':
    case 'error':
    case 'cancelled':
      return toModels(failActiveStep(base));
    case 'ok':
      return toModels(base);
  }
}

function baseSteps(step: PairStep | null): StepStates {
  switch (step) {
    case null:
    case 'dialing':
      return ['active', 'pending', 'pending'];
    case 'verifying':
    case 'exchanging':
      return ['done', 'active', 'pending'];
    case 'waitingLocal':
      return ['done', 'done', 'active'];
  }
}

function failActiveStep(states: StepStates): StepStates {
  const next: StepStates = [...states];
  const activeIndex = next.indexOf('active');
  if (activeIndex !== -1) {
    next[activeIndex] = 'failed';
  }
  return next;
}

function toModels(states: StepStates): PairStepModel[] {
  return [
    { key: 'connect', state: states[0] },
    { key: 'verify', state: states[1] },
    { key: 'approve', state: states[2] },
  ];
}
