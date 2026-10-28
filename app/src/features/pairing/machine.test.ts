import type { PairOutcome } from '../../native/rscore/types';
import {
  initialPairState,
  pairReducer,
  pairSteps,
  type PairUiState,
} from './machine';

const progress = pairReducer(initialPairState, {
  type: 'pairing.progress',
  step: 'dialing',
});

function failedWith(outcome: PairOutcome): PairUiState {
  return pairReducer(progress, { type: 'local.outcome', outcome });
}

describe('pairing state machine', () => {
  it('starts with the 30 second local approval budget', () => {
    expect(initialPairState).toEqual({
      phase: 'starting',
      step: null,
      path: null,
      latencyMs: null,
      fp: null,
      timeoutS: 30,
      outcome: null,
    });
    expect(pairSteps(initialPairState)).toEqual([
      { key: 'connect', state: 'active' },
      { key: 'verify', state: 'pending' },
      { key: 'approve', state: 'pending' },
    ]);
  });

  it.each([
    ['dialing', ['active', 'pending', 'pending']],
    ['verifying', ['done', 'active', 'pending']],
    ['exchanging', ['done', 'active', 'pending']],
    ['waitingLocal', ['done', 'done', 'active']],
  ] as const)('maps progress step %s to the step list', (step, states) => {
    const state = pairReducer(initialPairState, {
      type: 'pairing.progress',
      step,
    });

    expect(state.phase).toBe('progress');
    expect(pairSteps(state).map(item => item.state)).toEqual(states);
  });

  it.each(['mitm', 'badCode', 'expired', 'locked'] as const)(
    'marks certificate verification failed for %s',
    status => {
      const outcome: PairOutcome =
        status === 'badCode' ? { status, attemptsLeft: 4 } : { status };
      expect(pairSteps(failedWith(outcome)).map(item => item.state)).toEqual([
        'done',
        'failed',
        'pending',
      ]);
    },
  );

  it.each(['denied', 'timeout'] as const)(
    'marks local approval failed for %s',
    status => {
      expect(pairSteps(failedWith({ status })).map(item => item.state)).toEqual(
        ['done', 'done', 'failed'],
      );
    },
  );

  it.each(['disabledOnPath', 'error', 'cancelled'] as const)(
    'marks the active step failed for %s',
    status => {
      const outcome: PairOutcome =
        status === 'error'
          ? { status, message: 'connection reset' }
          : { status };
      const state = pairReducer(
        pairReducer(initialPairState, {
          type: 'pairing.progress',
          step: 'verifying',
        }),
        { type: 'local.outcome', outcome },
      );

      expect(pairSteps(state).map(item => item.state)).toEqual([
        'done',
        'failed',
        'pending',
      ]);
    },
  );

  it('marks certificate verification failed when the method is disabled', () => {
    expect(
      pairSteps(failedWith({ status: 'methodDisabled' })).map(
        item => item.state,
      ),
    ).toEqual(['done', 'failed', 'pending']);
  });

  it('marks all steps done after success', () => {
    const state = pairReducer(initialPairState, {
      type: 'pairing.done',
      outcome: { status: 'ok', hostId: 'host-1', hostName: 'Studio Mac mini' },
    });

    expect(state.phase).toBe('success');
    expect(pairSteps(state).map(item => item.state)).toEqual([
      'done',
      'done',
      'done',
    ]);
  });

  it('keeps the fingerprint and earlier optional values across progress events', () => {
    const verifying = pairReducer(progress, {
      type: 'pairing.progress',
      step: 'verifying',
      fp: 'fingerprint',
      path: 'tailnetDirect',
      latencyMs: 7,
    });
    const waiting = pairReducer(verifying, {
      type: 'pairing.progress',
      step: 'waitingLocal',
    });

    expect(waiting).toMatchObject({
      phase: 'progress',
      step: 'waitingLocal',
      fp: 'fingerprint',
      path: 'tailnetDirect',
      latencyMs: 7,
    });
  });

  it('uses a progress timeout override', () => {
    const state = pairReducer(initialPairState, {
      type: 'pairing.progress',
      step: 'waitingLocal',
      timeoutS: 25,
    });
    expect(state.timeoutS).toBe(25);
  });

  it('ignores the later promise result after pairing.done', () => {
    const done = pairReducer(initialPairState, {
      type: 'pairing.done',
      outcome: { status: 'ok', hostId: 'host-1', hostName: 'Studio Mac mini' },
    });
    const afterLateResult = pairReducer(done, {
      type: 'local.outcome',
      outcome: { status: 'ok', hostId: 'host-1', hostName: 'Studio Mac mini' },
    });

    expect(afterLateResult).toBe(done);
  });

  it('uses local.outcome when the promise settles first', () => {
    const state = pairReducer(initialPairState, {
      type: 'local.outcome',
      outcome: { status: 'denied' },
    });

    expect(state.phase).toBe('failed');
    expect(state.outcome).toEqual({ status: 'denied' });
    expect(
      pairReducer(state, {
        type: 'pairing.done',
        outcome: {
          status: 'ok',
          hostId: 'host-1',
          hostName: 'Studio Mac mini',
        },
      }),
    ).toBe(state);
  });
});
