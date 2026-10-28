import type { SshRecord } from '../sessions/sshStore';
import type { SshServer } from '../../native/rscore/types';
import { sshServers } from '../../native/rscore/fixtures';
import type { TabsState, TermTab } from './tabs';
import {
  failureKind,
  initTabs,
  matchTab,
  shouldSendPrefill,
  tabPhase,
  tabTitle,
  tabsReducer,
  termStatus,
} from './tabs';

function record(
  sshId: string,
  serverId: string,
  state: SshRecord['state'] = 'open',
  updatedAtUs = 1,
): SshRecord {
  return {
    sshId,
    serverId,
    state,
    openedAtUs: state === 'open' ? updatedAtUs : null,
    updatedAtUs,
  };
}

function tab(overrides: Partial<TermTab> = {}): TermTab {
  return {
    key: 't1',
    serverId: 'prod-api-01',
    sshId: 'ssh1',
    state: 'open',
    message: null,
    openError: null,
    pendingCheck: false,
    prefill: null,
    prefillSent: false,
    reopened: false,
    ...overrides,
  };
}

function live(): SshRecord[] {
  return [record('ssh1', 'a', 'open', 1), record('ssh2', 'b', 'open', 2)];
}

function find(state: TabsState, key: string): TermTab {
  const found = state.tabs.find(item => item.key === key);
  if (found === undefined) {
    throw new Error(`missing tab ${key}`);
  }
  return found;
}

describe('initTabs', () => {
  it('opens one opening tab when nothing is live', () => {
    const state = initTabs({ serverId: 'a' }, []);

    expect(state.tabs).toHaveLength(1);
    expect(state.tabs[0]).toMatchObject({
      key: 't1',
      serverId: 'a',
      sshId: null,
      state: null,
    });
    expect(state.activeKey).toBe('t1');
    expect(tabPhase(state.tabs[0])).toBe('opening');
  });

  it.each<[string, { serverId: string; sshId?: string; prefill?: string }]>([
    [
      'attaches all live tabs and activates the requested sshId',
      { serverId: 'b', sshId: 'ssh2' },
    ],
    ['reuses the earliest live tab for the same server', { serverId: 'a' }],
    [
      'opens a new tab when prefill is present',
      { serverId: 'a', prefill: 'curl example.com' },
    ],
  ])('%s', (_name, params) => {
    const state = initTabs(params, live());

    if (params.prefill !== undefined) {
      expect(state.tabs).toHaveLength(3);
      expect(state.activeKey).toBe('t3');
      expect(find(state, 't3').prefill).toBe(params.prefill);
      return;
    }

    expect(state.tabs).toHaveLength(2);
    expect(state.tabs.map(item => tabPhase(item))).toEqual(['open', 'open']);
    expect(state.activeKey).toBe(params.sshId === 'ssh2' ? 't2' : 't1');
  });
});

describe('tabsReducer', () => {
  it('does not attach the same sshId twice', () => {
    const first = tabsReducer(
      { tabs: [], activeKey: null, seq: 0 },
      { type: 'attach', serverId: 'a', sshId: 'ssh1', state: 'open' },
    );
    const second = tabsReducer(first, {
      type: 'attach',
      serverId: 'a',
      sshId: 'ssh1',
      state: 'authenticating',
      activate: true,
    });

    expect(second.tabs).toHaveLength(1);
    expect(second.activeKey).toBe('t1');
  });

  it('records open, failure and reopen transitions', () => {
    let state = initTabs({ serverId: 'a' }, []);
    state = tabsReducer(state, { type: 'opened', key: 't1', sshId: 'ssh7' });
    expect(find(state, 't1').sshId).toBe('ssh7');
    expect(tabPhase(find(state, 't1'))).toBe('connecting');

    state = tabsReducer(state, {
      type: 'openFailed',
      key: 't1',
      message: 'no route',
    });
    expect(tabPhase(find(state, 't1'))).toBe('failed');

    expect(find(state, 't1').reopened).toBe(false);

    state = tabsReducer(state, { type: 'reopen', key: 't1' });
    expect(find(state, 't1')).toMatchObject({
      sshId: null,
      state: null,
      message: null,
      openError: null,
      pendingCheck: false,
      reopened: true,
    });
    expect(tabPhase(find(state, 't1'))).toBe('opening');
  });

  it('copies ssh state into the tab, including failed and closed messages', () => {
    let state = initTabs({ serverId: 'a', sshId: 'ssh1' }, [
      record('ssh1', 'a'),
    ]);
    state = tabsReducer(state, {
      type: 'sshState',
      sshId: 'ssh1',
      state: 'failed',
      message: 'auth failed',
    });

    const failed = find(state, 't1');
    expect(tabPhase(failed)).toBe('failed');
    expect(termStatus(failed, undefined, true)).toMatchObject({
      message: 'auth failed',
      canReconnect: true,
      tone: 'bad',
    });

    state = tabsReducer(state, {
      type: 'sshState',
      sshId: 'ssh1',
      state: 'closed',
    });
    expect(tabPhase(find(state, 't1'))).toBe('closed');
  });

  it('tracks the Tailscale check round trip', () => {
    let state = initTabs({ serverId: 'a', sshId: 'ssh1' }, [
      record('ssh1', 'a', 'authenticating'),
    ]);
    state = tabsReducer(state, { type: 'authUrl', sshId: 'ssh1' });
    state = tabsReducer(state, {
      type: 'sshState',
      sshId: 'ssh1',
      state: 'authenticating',
      message: 'confirm in browser',
    });
    expect(find(state, 't1').pendingCheck).toBe(true);
    expect(tabPhase(find(state, 't1'))).toBe('check');

    state = tabsReducer(state, {
      type: 'sshState',
      sshId: 'ssh1',
      state: 'open',
    });
    expect(find(state, 't1').pendingCheck).toBe(false);
    expect(tabPhase(find(state, 't1'))).toBe('open');
  });

  it('activates the next tab to the right when closing the active tab', () => {
    let state = initTabs({ serverId: 'a' }, []);
    state = tabsReducer(state, { type: 'open', serverId: 'b' });
    state = tabsReducer(state, { type: 'open', serverId: 'c' });
    state = tabsReducer(state, { type: 'activate', key: 't2' });

    state = tabsReducer(state, { type: 'close', key: 't2' });
    expect(state.tabs.map(item => item.key)).toEqual(['t1', 't3']);
    expect(state.activeKey).toBe('t3');
  });

  it('activates the tab to the left when closing the rightmost tab, then clears the last close', () => {
    let state = initTabs({ serverId: 'a' }, []);
    state = tabsReducer(state, { type: 'open', serverId: 'b' });
    state = tabsReducer(state, { type: 'close', key: 't2' });
    expect(state.activeKey).toBe('t1');

    state = tabsReducer(state, { type: 'close', key: 't1' });
    expect(state.activeKey).toBeNull();
    expect(state.tabs).toEqual([]);
  });
});

describe('matchTab', () => {
  it('prefers a tab with the same sshId', () => {
    const state = initTabs({ serverId: 'a', sshId: 'ssh1' }, live());
    expect(matchTab(state, 'ssh1', 'a')?.key).toBe('t1');
  });

  it('matches an opening tab for the same server', () => {
    const state = initTabs({ serverId: 'a' }, []);
    expect(matchTab(state, 'ssh7', 'a')?.key).toBe('t1');
  });

  it('returns null for another server and skips tabs with an open error', () => {
    const failed = tabsReducer(initTabs({ serverId: 'a' }, []), {
      type: 'openFailed',
      key: 't1',
      message: 'no route',
    });
    expect(matchTab(failed, 'ssh7', 'a')).toBeNull();
    expect(matchTab(initTabs({ serverId: 'a' }, []), 'ssh7', 'b')).toBeNull();
  });
});

describe('termStatus', () => {
  it('describes an open Tailscale SSH tab with tmux', () => {
    const server = sshServers.find(item => item.serverId === 'prod-api-01');
    expect(server).toBeDefined();

    expect(termStatus(tab(), server, true)).toEqual({
      phase: 'open',
      auth: 'tailscale',
      jump: null,
      tmux: true,
      resumed: false,
      message: null,
      canReconnect: false,
      tone: 'good',
    });
    expect(termStatus(tab(), server, false).tmux).toBe(false);
  });

  it('only calls a tmux tab resumed after a reconnect has opened', () => {
    // T07: the first connection has nothing to resume.
    expect(termStatus(tab({ reopened: true }), undefined, true).resumed).toBe(
      true,
    );
    expect(termStatus(tab({ reopened: true }), undefined, false).resumed).toBe(
      false,
    );
    expect(
      termStatus(tab({ reopened: true, state: 'connecting' }), undefined, true)
        .resumed,
    ).toBe(false);
  });

  it('reads jump hosts and uses muted for connecting tabs', () => {
    const server = sshServers.find(
      item => item.serverId === 'manual-build-srv',
    );
    expect(
      termStatus(tab({ serverId: 'manual-build-srv' }), server, true).jump,
    ).toBe('jump-01');
    expect(
      termStatus(tab({ sshId: null, state: null }), undefined, true),
    ).toMatchObject({ tone: 'muted', phase: 'opening', auth: null });
  });
});

describe('helpers', () => {
  it('only sends a non-empty prefill once the tab is open', () => {
    expect(shouldSendPrefill(tab({ prefill: 'ls' }))).toBe(true);
    expect(shouldSendPrefill(tab({ prefill: 'ls', prefillSent: true }))).toBe(
      false,
    );
    expect(shouldSendPrefill(tab({ prefill: 'ls', state: 'connecting' }))).toBe(
      false,
    );
  });

  it('titles a tab with its server name or falls back to serverId', () => {
    const servers: SshServer[] = [sshServers[3]];
    expect(tabTitle(tab({ serverId: 'manual-build-srv' }), servers)).toBe(
      'build-srv',
    );
    expect(tabTitle(tab({ serverId: 'missing' }), servers)).toBe('missing');
  });

  it('numbers tabs that share a server from the second one', () => {
    const servers: SshServer[] = [sshServers[3]];
    const first = tab({ key: 't1', serverId: 'manual-build-srv' });
    const second = tab({ key: 't2', serverId: 'manual-build-srv' });
    const other = tab({ key: 't3', serverId: 'missing' });
    const tabs = [first, other, second];
    expect(tabTitle(first, servers, tabs)).toBe('build-srv');
    expect(tabTitle(second, servers, tabs)).toBe('build-srv 2');
    expect(tabTitle(other, servers, tabs)).toBe('missing');
  });

  it.each([
    ['Permission denied (publickey)', 'auth'],
    ['authentication failed', 'auth'],
    ['connection timed out', 'network'],
    ['no route to host', 'network'],
    ['Connection refused', 'network'],
    ['channel closed unexpectedly', 'other'],
  ] as const)('sorts %j as a %s failure', (message, kind) => {
    expect(failureKind(message)).toBe(kind);
  });

  it('has no failure kind without a message', () => {
    expect(failureKind(null)).toBeNull();
    expect(failureKind('  ')).toBeNull();
  });
});
