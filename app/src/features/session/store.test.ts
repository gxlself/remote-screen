import type { CoreEvent, SessionSummary } from '../../native/rscore/types';
import { applySessionEvent, createSessionsStore, selectLive } from './store';
import type { SessionRecord, SessionsState } from './store';

const NOW = 1_800_000_000_000_000;
const SESSION_ID = 's1';
const HOST_ID = 'h1';

const summary: SessionSummary = {
  id: SESSION_ID,
  hostId: HOST_ID,
  hostName: 'Studio Mac mini',
  phase: 'connecting',
  role: null,
  path: null,
  transport: null,
  startedAt: NOW,
  displayId: null,
};

function seeded(): SessionsState {
  return { [SESSION_ID]: record() };
}

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...summary,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays: [],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
    ...overrides,
  };
}

const participant = {
  deviceId: 'v1',
  deviceName: 'iPad',
  user: 'user@example.com',
  path: 'tailnetDirect' as const,
  since: NOW,
};

type Case = {
  name: string;
  event: CoreEvent;
  initial?: SessionsState;
  expected: (next: SessionsState, before: SessionsState) => void;
};

const cases: Case[] = [
  {
    name: 'session.state creates a missing record with hostId as hostName',
    event: {
      type: 'session.state',
      sessionId: 'new-session',
      hostId: 'new-host',
      phase: 'connecting',
      dial: [{ path: 'lan', addr: '192.168.31.20', state: 'trying' }],
      approvalTimeoutS: 30,
    },
    initial: {},
    expected: next => {
      expect(next['new-session']).toMatchObject({
        id: 'new-session',
        hostId: 'new-host',
        hostName: 'new-host',
        phase: 'connecting',
        startedAt: NOW,
        approvalTimeoutS: 30,
        dial: [{ path: 'lan', addr: '192.168.31.20', state: 'trying' }],
      });
    },
  },
  {
    name: 'session.state updates connection fields',
    event: {
      type: 'session.state',
      sessionId: SESSION_ID,
      hostId: HOST_ID,
      phase: 'reconnecting',
      path: 'tailnetDerp',
      transport: 'tcp',
      derpRegion: 'Tokyo',
      dial: [
        {
          path: 'tailnetDerp',
          addr: '100.64.0.7',
          state: 'ok',
          ms: 48,
        },
      ],
      approvalTimeoutS: 15,
      reconnectReason: 'pathLost',
    },
    expected: next => {
      expect(next[SESSION_ID]).toMatchObject({
        phase: 'reconnecting',
        path: 'tailnetDerp',
        transport: 'tcp',
        derpRegion: 'Tokyo',
        approvalTimeoutS: 15,
        reconnectReason: 'pathLost',
      });
    },
  },
  {
    name: 'session.state streaming clears reconnect reason and idle warning',
    event: {
      type: 'session.state',
      sessionId: SESSION_ID,
      hostId: HOST_ID,
      phase: 'streaming',
      reconnectReason: 'resume',
    },
    initial: {
      [SESSION_ID]: record({
        phase: 'reconnecting',
        reconnectReason: 'pathLost',
        idleWarning: { secondsLeft: 45, lockOnEnd: true, atUs: NOW },
      }),
    },
    expected: next => {
      expect(next[SESSION_ID].reconnectReason).toBeNull();
      expect(next[SESSION_ID].idleWarning).toBeNull();
    },
  },
  {
    name: 'session.role updates participants and clears control request for controller',
    event: {
      type: 'session.role',
      sessionId: SESSION_ID,
      role: 'controller',
      controller: participant,
      viewers: [{ ...participant, deviceId: 'v2' }],
      inputPausedLocal: true,
    },
    initial: {
      [SESSION_ID]: record({
        controlRequest: {
          requester: participant,
          timeoutS: 30,
          atUs: NOW,
        },
      }),
    },
    expected: next => {
      expect(next[SESSION_ID]).toMatchObject({
        role: 'controller',
        controller: participant,
        inputPausedLocal: true,
        controlRequest: null,
      });
      expect(next[SESSION_ID].viewers).toHaveLength(1);
    },
  },
  {
    name: 'session.stats saves stats and connection metadata',
    event: {
      type: 'session.stats',
      sessionId: SESSION_ID,
      rttMs: 6,
      fps: 60,
      bitrateKbps: 8000,
      decodeMs: 4,
      lossPct: 0,
      width: 2560,
      height: 1600,
      codec: 'hevc',
      path: 'intranet',
      transport: 'quic',
      derpRegion: 'Shanghai',
    },
    expected: next => {
      expect(next[SESSION_ID].stats?.rttMs).toBe(6);
      expect(next[SESSION_ID].stats?.fps).toBe(60);
      expect(next[SESSION_ID]).toMatchObject({
        path: 'intranet',
        transport: 'quic',
        derpRegion: 'Shanghai',
      });
    },
  },
  {
    name: 'session.displays updates displays and current display id',
    event: {
      type: 'session.displays',
      sessionId: SESSION_ID,
      displays: [
        {
          id: 2,
          name: 'DELL U2723QE',
          widthPx: 3840,
          heightPx: 2160,
          primary: false,
        },
      ],
      currentId: 2,
    },
    expected: next => {
      expect(next[SESSION_ID].displays).toHaveLength(1);
      expect(next[SESSION_ID].displayId).toBe(2);
    },
  },
  {
    name: 'session.clipboard prepends an item',
    event: {
      type: 'session.clipboard',
      sessionId: SESSION_ID,
      direction: 'fromHost',
      text: 'first',
      at: NOW,
    },
    expected: next => {
      expect(next[SESSION_ID].clipboard).toEqual([
        { direction: 'fromHost', text: 'first', at: NOW },
      ]);
    },
  },
  {
    name: 'session.idleWarning records the countdown and receipt time',
    event: {
      type: 'session.idleWarning',
      sessionId: SESSION_ID,
      secondsLeft: 60,
      lockOnEnd: true,
    },
    expected: next => {
      expect(next[SESSION_ID].idleWarning).toEqual({
        secondsLeft: 60,
        lockOnEnd: true,
        atUs: NOW,
      });
    },
  },
  {
    name: 'session.controlRequest records the requester and deadline',
    event: {
      type: 'session.controlRequest',
      sessionId: SESSION_ID,
      requester: participant,
      timeoutS: 30,
    },
    expected: next => {
      expect(next[SESSION_ID].controlRequest).toEqual({
        requester: participant,
        timeoutS: 30,
        atUs: NOW,
      });
    },
  },
  {
    name: 'session.error ends the session and retains the error',
    event: {
      type: 'session.error',
      sessionId: SESSION_ID,
      hostId: HOST_ID,
      code: 140,
      reason: 'userClosed',
      detail: { console: 'other_user' },
      message: 'denied',
    },
    expected: next => {
      expect(next[SESSION_ID].phase).toBe('ended');
      expect(next[SESSION_ID].lastError).toEqual({
        code: 140,
        reason: 'userClosed',
        detail: { console: 'other_user' },
        message: 'denied',
      });
    },
  },
  {
    name: 'session.error 141 keeps the session live',
    event: {
      type: 'session.error',
      sessionId: SESSION_ID,
      hostId: HOST_ID,
      code: 141,
      message: 'control denied',
    },
    initial: {
      [SESSION_ID]: record({ phase: 'streaming' }),
    },
    expected: next => {
      expect(next[SESSION_ID].phase).toBe('streaming');
      expect(next[SESSION_ID].lastError?.code).toBe(141);
    },
  },
  {
    name: 'unrelated events return the same state object',
    event: { type: 'pairing.done', outcome: { status: 'cancelled' } },
    expected: (next, before) => {
      expect(next).toBe(before);
    },
  },
];

describe('applySessionEvent', () => {
  for (const item of cases) {
    it(item.name, () => {
      const before = item.initial ?? seeded();
      const next = applySessionEvent(before, item.event, NOW);
      item.expected(next, before);
      expect(before).toBe(item.initial ?? before);
    });
  }
});

describe('session.error phase handling', () => {
  it('keeps a streaming session live for an error without a reason', () => {
    const state = applySessionEvent(
      { [SESSION_ID]: record({ phase: 'streaming' }) },
      {
        type: 'session.error',
        sessionId: SESSION_ID,
        hostId: HOST_ID,
        code: 101,
        detail: { action: 'ctrl_alt_del' },
      },
      NOW,
    );
    expect(state[SESSION_ID].phase).toBe('streaming');
    expect(state[SESSION_ID].lastError?.code).toBe(101);
    expect(selectLive(state).map(item => item.id)).toContain(SESSION_ID);
  });

  it('ends the streaming session only when session.state says ended', () => {
    let state = applySessionEvent(
      { [SESSION_ID]: record({ phase: 'streaming' }) },
      {
        type: 'session.error',
        sessionId: SESSION_ID,
        hostId: HOST_ID,
        code: 101,
        detail: { action: 'ctrl_alt_del' },
      },
      NOW,
    );
    state = applySessionEvent(
      state,
      {
        type: 'session.state',
        sessionId: SESSION_ID,
        hostId: HOST_ID,
        phase: 'ended',
      },
      NOW + 1,
    );
    expect(state[SESSION_ID].phase).toBe('ended');
    expect(state[SESSION_ID].lastError?.code).toBe(101);
  });

  it('ends a streaming session when the error carries a reason', () => {
    const state = applySessionEvent(
      { [SESSION_ID]: record({ phase: 'streaming' }) },
      {
        type: 'session.error',
        sessionId: SESSION_ID,
        hostId: HOST_ID,
        code: 140,
        reason: 'userClosed',
      },
      NOW,
    );
    expect(state[SESSION_ID].phase).toBe('ended');
  });

  it('ends a connecting session on an error without a reason', () => {
    const state = applySessionEvent(
      { [SESSION_ID]: record({ phase: 'connecting' }) },
      {
        type: 'session.error',
        sessionId: SESSION_ID,
        hostId: HOST_ID,
        code: 140,
      },
      NOW,
    );
    expect(state[SESSION_ID].phase).toBe('ended');
  });
});

it('keeps only the newest five clipboard entries', () => {
  let state = seeded();
  for (let index = 1; index <= 6; index += 1) {
    state = applySessionEvent(
      state,
      {
        type: 'session.clipboard',
        sessionId: SESSION_ID,
        direction: 'toHost',
        text: `copy-${index}`,
        at: NOW + index,
      },
      NOW + index,
    );
  }
  expect(state[SESSION_ID].clipboard).toHaveLength(5);
  expect(state[SESSION_ID].clipboard[0]?.text).toBe('copy-6');
  expect(state[SESSION_ID].clipboard[4]?.text).toBe('copy-2');
});

it('keeps ended records out of selectLive until dismissal', () => {
  let state = seeded();
  state = applySessionEvent(
    state,
    {
      type: 'session.error',
      sessionId: SESSION_ID,
      hostId: HOST_ID,
      code: 140,
    },
    NOW,
  );
  expect(selectLive(state)).toEqual([]);
  expect(state[SESSION_ID].lastError?.code).toBe(140);
});

it('sorts live sessions by start time', () => {
  const state: SessionsState = {
    later: record({ id: 'later', startedAt: NOW + 2 }),
    earlier: record({ id: 'earlier', startedAt: NOW + 1 }),
    ended: record({ id: 'ended', startedAt: NOW, phase: 'ended' }),
  };
  expect(selectLive(state).map(item => item.id)).toEqual(['earlier', 'later']);
});

it('clears idle and control prompts and ignores unknown ids', () => {
  const store = createSessionsStore();
  store.setState({
    [SESSION_ID]: record({
      idleWarning: { secondsLeft: 5, lockOnEnd: false, atUs: NOW },
      controlRequest: {
        requester: participant,
        timeoutS: 30,
        atUs: NOW,
      },
    }),
  });

  store.clearIdleWarning(SESSION_ID);
  store.clearControlRequest(SESSION_ID);
  expect(store.getState()[SESSION_ID].idleWarning).toBeNull();
  expect(store.getState()[SESSION_ID].controlRequest).toBeNull();
  expect(() => store.clearIdleWarning('missing')).not.toThrow();
  expect(() => store.clearControlRequest('missing')).not.toThrow();
});

it('seeds summaries and dismisses records', () => {
  const store = createSessionsStore();
  store.seed([summary]);
  expect(store.getState()[SESSION_ID].hostName).toBe(summary.hostName);

  store.dismiss(SESSION_ID);
  expect(store.getState()[SESSION_ID]).toBeUndefined();
  expect(() => store.dismiss('missing')).not.toThrow();
});
