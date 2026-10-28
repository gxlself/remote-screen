import type { CoreEvent } from '../../native/rscore/types';
import {
  LIVE_SSH_STATES,
  applySshEvent,
  createSshStore,
  selectLiveSsh,
  sshStore,
} from './sshStore';
import type { SshRecord, SshState } from './sshStore';

function sshEvent(
  state: SshRecord['state'],
  overrides: Partial<Extract<CoreEvent, { type: 'ssh.state' }>> = {},
): Extract<CoreEvent, { type: 'ssh.state' }> {
  return {
    type: 'ssh.state',
    sshId: 'ssh-1',
    serverId: 'gpu-train-02',
    state,
    ...overrides,
  };
}

describe('applySshEvent', () => {
  it('creates a record for an initially connecting session', () => {
    const next = applySshEvent({}, sshEvent('connecting'), 100);
    expect(next['ssh-1']).toEqual({
      sshId: 'ssh-1',
      serverId: 'gpu-train-02',
      state: 'connecting',
      openedAtUs: null,
      updatedAtUs: 100,
    });
  });

  it('records openedAtUs on the first open and keeps it on later opens', () => {
    const connecting = applySshEvent({}, sshEvent('connecting'), 100);
    const opened = applySshEvent(connecting, sshEvent('open'), 200);
    expect(opened['ssh-1'].openedAtUs).toBe(200);

    const reopened = applySshEvent(opened, sshEvent('open'), 300);
    expect(reopened['ssh-1'].openedAtUs).toBe(200);
  });

  it('updates closed state while retaining the record', () => {
    const opened = applySshEvent({}, sshEvent('open'), 100);
    const closed = applySshEvent(opened, sshEvent('closed'), 200);
    expect(closed['ssh-1']).toMatchObject({
      state: 'closed',
      openedAtUs: 100,
      updatedAtUs: 200,
    });
  });

  it('keeps the message from the latest event', () => {
    const failed = applySshEvent(
      {},
      sshEvent('failed', { message: 'host key changed' }),
      100,
    );
    expect(failed['ssh-1'].message).toBe('host key changed');
  });

  it('returns the same reference for unrelated events', () => {
    const state: SshState = {};
    const event: CoreEvent = {
      type: 'network.changed',
      status: { ifaces: [] },
    };
    expect(applySshEvent(state, event, 100)).toBe(state);
  });
});

describe('selectLiveSsh', () => {
  it('contains only connecting, authenticating, and open records', () => {
    expect(LIVE_SSH_STATES).toEqual(['connecting', 'authenticating', 'open']);
    const state: SshState = {};
    for (const stateName of LIVE_SSH_STATES) {
      state[stateName] = {
        sshId: stateName,
        serverId: stateName,
        state: stateName,
        openedAtUs: null,
        updatedAtUs: 1,
      };
    }
    state.closed = {
      sshId: 'closed',
      serverId: 'closed',
      state: 'closed',
      openedAtUs: 1,
      updatedAtUs: 2,
    };
    state.failed = {
      sshId: 'failed',
      serverId: 'failed',
      state: 'failed',
      openedAtUs: 1,
      updatedAtUs: 3,
    };

    expect(selectLiveSsh(state).map(record => record.sshId)).toEqual([
      'connecting',
      'authenticating',
      'open',
    ]);
  });

  it('sorts by openedAtUs, then updatedAtUs', () => {
    const state: SshState = {
      opened: {
        sshId: 'opened',
        serverId: 's',
        state: 'open',
        openedAtUs: 20,
        updatedAtUs: 30,
      },
      connectingNew: {
        sshId: 'connectingNew',
        serverId: 's',
        state: 'connecting',
        openedAtUs: null,
        updatedAtUs: 40,
      },
      connectingOld: {
        sshId: 'connectingOld',
        serverId: 's',
        state: 'authenticating',
        openedAtUs: null,
        updatedAtUs: 10,
      },
    };
    expect(selectLiveSsh(state).map(record => record.sshId)).toEqual([
      'connectingOld',
      'opened',
      'connectingNew',
    ]);
  });
});

describe('createSshStore', () => {
  afterEach(() => {
    sshStore.setState({}, true);
  });

  it('applies events and removes records through its actions', () => {
    const store = createSshStore();
    store.apply(sshEvent('open'), 100);
    expect(store.getState()['ssh-1'].openedAtUs).toBe(100);
    store.remove('ssh-1');
    expect(store.getState()).toEqual({});
  });
});
