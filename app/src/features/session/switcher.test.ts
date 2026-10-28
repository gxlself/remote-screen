import type { CollapsedAdapter } from '../adapters/liveStore';
import type { SshRecord } from '../sessions/sshStore';
import type { SessionRecord } from './store';
import {
  MANY_SESSIONS,
  adapterSwitcherItem,
  adapterSwitcherStatus,
  buildSwitcherList,
  hostSwitcherItem,
  hostSwitcherStatus,
  sessionCount,
  shortcutTarget,
  showsManyNote,
  sshSwitcherItem,
} from './switcher';
import type { SwitcherItem } from './switcher';

function host(id: string, startedAtUs: number): SwitcherItem {
  return {
    kind: 'host',
    id,
    hostId: `host-${id}`,
    name: `Host ${id}`,
    status: 'connected',
    startedAtUs,
  };
}

function screenOf(
  id: string,
  startedAtUs: number,
  protocol: 'vnc' | 'rdp' = 'vnc',
): SwitcherItem {
  return {
    kind: protocol,
    id,
    params: { target: { protocol, addr: `10.0.0.${id}` }, name: `PC ${id}` },
    name: `PC ${id}`,
    status: 'connected',
    startedAtUs,
  };
}

function terminal(id: string, startedAtUs: number): SwitcherItem {
  return {
    kind: 'ssh',
    id,
    serverId: `server-${id}`,
    name: `Server ${id}`,
    status: 'connected',
    startedAtUs,
  };
}

const ids = (items: readonly SwitcherItem[]) => items.map(item => item.id);

describe('buildSwitcherList', () => {
  it('orders screens by start time and keeps terminals apart', () => {
    const list = buildSwitcherList([
      screenOf('3', 300),
      terminal('t1', 50),
      host('1', 100),
      screenOf('2', 200, 'rdp'),
    ]);
    expect(ids(list.screens)).toEqual(['1', '2', '3']);
    expect(ids(list.terminals)).toEqual(['t1']);
    expect(sessionCount(list)).toBe(4);
  });

  it('keeps the numbers stable: same start time falls back to the id', () => {
    const first = buildSwitcherList([screenOf('b', 100), host('a', 100)]);
    const second = buildSwitcherList([host('a', 100), screenOf('b', 100)]);
    expect(ids(first.screens)).toEqual(['a', 'b']);
    expect(ids(second.screens)).toEqual(['a', 'b']);
  });

  it("puts the page's own session in, replacing a stale copy", () => {
    const stale = { ...screenOf('2', 200), status: 'connecting' as const };
    const current = screenOf('2', 200);
    const list = buildSwitcherList([host('1', 100), stale], current);
    expect(list.screens).toEqual([host('1', 100), current]);

    expect(ids(buildSwitcherList([], current).screens)).toEqual(['2']);
    expect(buildSwitcherList([], null)).toEqual({ screens: [], terminals: [] });
  });
});

describe('items from the stores', () => {
  it('names the status of each kind of session', () => {
    expect(hostSwitcherStatus('streaming')).toBe('connected');
    expect(hostSwitcherStatus('reconnecting')).toBe('reconnecting');
    expect(hostSwitcherStatus('connecting')).toBe('connecting');
    expect(hostSwitcherStatus('pendingApproval')).toBe('connecting');
    expect(hostSwitcherStatus('ended')).toBe('disconnected');
    expect(adapterSwitcherStatus('streaming')).toBe('connected');
    expect(adapterSwitcherStatus('streaming', true)).toBe('reconnecting');
    expect(adapterSwitcherStatus('authenticating')).toBe('connecting');
    expect(adapterSwitcherStatus('opening')).toBe('connecting');
    expect(adapterSwitcherStatus('ended')).toBe('disconnected');
  });

  it('builds host, adapter and terminal items', () => {
    const record = {
      id: 's1',
      hostId: 'h1',
      hostName: 'h1',
      phase: 'reconnecting',
      startedAt: 10,
    } as SessionRecord;
    expect(hostSwitcherItem(record, 'Studio')).toEqual({
      kind: 'host',
      id: 's1',
      hostId: 'h1',
      name: 'Studio',
      status: 'reconnecting',
      startedAtUs: 10,
    });

    const params = {
      target: { protocol: 'rdp' as const, addr: '10.0.0.9' },
      name: 'Win PC',
    };
    const collapsed = {
      sessionId: 'a1',
      params,
      name: 'Win PC',
      phase: 'streaming',
      startedAtUs: 20,
    } as CollapsedAdapter;
    expect(adapterSwitcherItem(collapsed)).toEqual({
      kind: 'rdp',
      id: 'a1',
      params,
      name: 'Win PC',
      status: 'connected',
      startedAtUs: 20,
    });

    const ssh: SshRecord = {
      sshId: 'x1',
      serverId: 'srv',
      state: 'authenticating',
      openedAtUs: null,
      updatedAtUs: 30,
    };
    expect(sshSwitcherItem(ssh, 'NAS')).toEqual({
      kind: 'ssh',
      id: 'x1',
      serverId: 'srv',
      name: 'NAS',
      status: 'connecting',
      startedAtUs: 30,
    });
    expect(
      sshSwitcherItem({ ...ssh, state: 'open', openedAtUs: 25 }, 'NAS'),
    ).toMatchObject({ status: 'connected', startedAtUs: 25 });
  });
});

describe('shortcutTarget', () => {
  const screens = [host('1', 100), screenOf('2', 200), screenOf('3', 300)];

  it('next and previous go round', () => {
    expect(shortcutTarget(screens, '1', { action: 'next' })?.id).toBe('2');
    expect(shortcutTarget(screens, '3', { action: 'next' })?.id).toBe('1');
    expect(shortcutTarget(screens, '1', { action: 'previous' })?.id).toBe('3');
    expect(shortcutTarget(screens, '2', { action: 'previous' })?.id).toBe('1');
  });

  it('a number goes to that session', () => {
    expect(
      shortcutTarget(screens, '1', { action: 'index', index: 3 })?.id,
    ).toBe('3');
    expect(
      shortcutTarget(screens, '3', { action: 'index', index: 1 })?.id,
    ).toBe('1');
  });

  it('ignores numbers past the list and the session already shown', () => {
    expect(
      shortcutTarget(screens, '1', { action: 'index', index: 4 }),
    ).toBeNull();
    expect(
      shortcutTarget(screens, '1', { action: 'index', index: 9 }),
    ).toBeNull();
    expect(
      shortcutTarget(screens, '2', { action: 'index', index: 2 }),
    ).toBeNull();
  });

  it('does nothing alone or for the switcher shortcut', () => {
    const alone = [screenOf('2', 200)];
    expect(shortcutTarget(alone, '2', { action: 'next' })).toBeNull();
    expect(shortcutTarget(alone, '2', { action: 'previous' })).toBeNull();
    expect(shortcutTarget([], null, { action: 'next' })).toBeNull();
    expect(shortcutTarget(screens, '1', { action: 'switcher' })).toBeNull();
  });

  it('starts from the ends while the page has no session yet', () => {
    expect(shortcutTarget(screens, null, { action: 'next' })?.id).toBe('1');
    expect(shortcutTarget(screens, null, { action: 'previous' })?.id).toBe(
      '3',
    );
  });
});

it(`notes the battery above ${MANY_SESSIONS} open sessions, terminals included`, () => {
  const four = buildSwitcherList([
    host('1', 1),
    screenOf('2', 2),
    screenOf('3', 3),
    terminal('t', 4),
  ]);
  expect(showsManyNote(four)).toBe(false);
  const five = buildSwitcherList([
    ...four.screens,
    ...four.terminals,
    screenOf('5', 5),
  ]);
  expect(sessionCount(five)).toBe(5);
  expect(showsManyNote(five)).toBe(true);
});
