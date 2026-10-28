import { connectSteps } from './connectSteps';

describe('connectSteps', () => {
  it.each([
    {
      name: 'starts the tunnel before any later phase',
      input: {
        phase: 'connecting' as const,
        startedAtMs: 0,
        phaseAtMs: {},
        firstFrameAtMs: null,
        nowMs: 300,
      },
      expected: {
        steps: [
          { key: 'tunnel', state: 'active', seconds: 0.3 },
          { key: 'auth', state: 'pending', seconds: null },
          { key: 'firstFrame', state: 'pending', seconds: null },
        ],
        elapsedS: 0.3,
        stuck: null,
      },
    },
    {
      name: 'shows done tunnel and auth while waiting for the first frame',
      input: {
        phase: 'streaming' as const,
        startedAtMs: 0,
        phaseAtMs: { authenticating: 400, streaming: 700 },
        firstFrameAtMs: null,
        nowMs: 3400,
      },
      expected: {
        steps: [
          { key: 'tunnel', state: 'done', seconds: 0.4 },
          { key: 'auth', state: 'done', seconds: 0.3 },
          { key: 'firstFrame', state: 'active', seconds: 2.7 },
        ],
        elapsedS: 3.4,
        // 3 秒多还算正常，不提示「还在等」。
        stuck: null,
      },
    },
    {
      name: 'marks the first-frame step stuck after 8 seconds',
      input: {
        phase: 'streaming' as const,
        startedAtMs: 0,
        phaseAtMs: { authenticating: 400, streaming: 700 },
        firstFrameAtMs: null,
        nowMs: 8000,
      },
      expected: {
        steps: [
          { key: 'tunnel', state: 'done', seconds: 0.4 },
          { key: 'auth', state: 'done', seconds: 0.3 },
          { key: 'firstFrame', state: 'active', seconds: 7.3 },
        ],
        elapsedS: 8,
        stuck: 'firstFrame',
      },
    },
    {
      name: 'completes every step after the first frame',
      input: {
        phase: 'streaming' as const,
        startedAtMs: 0,
        phaseAtMs: { authenticating: 400, streaming: 700 },
        firstFrameAtMs: 3600,
        nowMs: 4000,
      },
      expected: {
        steps: [
          { key: 'tunnel', state: 'done', seconds: 0.4 },
          { key: 'auth', state: 'done', seconds: 0.3 },
          { key: 'firstFrame', state: 'done', seconds: 2.9 },
        ],
        elapsedS: 4,
        stuck: null,
      },
    },
    {
      name: 'finishes auth at streaming when authenticating was not reported',
      input: {
        phase: 'streaming' as const,
        startedAtMs: 0,
        phaseAtMs: { streaming: 500 },
        firstFrameAtMs: null,
        nowMs: 900,
      },
      expected: {
        steps: [
          { key: 'tunnel', state: 'done', seconds: 0.5 },
          { key: 'auth', state: 'done', seconds: 0 },
          { key: 'firstFrame', state: 'active', seconds: 0.4 },
        ],
        elapsedS: 0.9,
        stuck: null,
      },
    },
    {
      name: 'finishes auth when approval is requested at 600 ms',
      input: {
        phase: 'pendingApproval' as const,
        startedAtMs: 0,
        phaseAtMs: { authenticating: 200, pendingApproval: 600 },
        firstFrameAtMs: null,
        nowMs: 900,
      },
      expected: {
        steps: [
          { key: 'tunnel', state: 'done', seconds: 0.2 },
          { key: 'auth', state: 'done', seconds: 0.4 },
          { key: 'firstFrame', state: 'pending', seconds: null },
        ],
        elapsedS: 0.9,
        stuck: null,
      },
    },
  ])('$name', ({ input, expected }) => {
    expect(connectSteps(input)).toEqual(expected);
  });
});
