import { CommonActions } from '@react-navigation/native';
import { resetArrival } from './spaces';
import type { SwitcherItem } from './switcher';
import { replaceWithSession } from './useSessionSwitcher';

const mac: SwitcherItem = {
  kind: 'vnc',
  id: 'adapter-2',
  params: { target: { protocol: 'vnc', addr: 'mac.local' }, name: 'Mac' },
  name: 'Mac',
  status: 'connected',
  startedAtUs: 2,
};

const host: SwitcherItem = {
  kind: 'host',
  id: 'host-1',
  hostId: 'h1',
  name: 'Studio',
  status: 'connected',
  startedAtUs: 3,
};

function fakeNavigation(top: { name: string; params?: object }) {
  const state = {
    key: 'root',
    index: 1,
    routeNames: ['MainTabs', top.name],
    routes: [
      { key: 'tabs', name: 'MainTabs' },
      { key: 'page', name: top.name, params: top.params },
    ],
    type: 'stack',
    stale: false,
  };
  return {
    getState: jest.fn(() => state),
    dispatch: jest.fn(),
    replace: jest.fn(),
  };
}

afterEach(resetArrival);

// The session pages are full-screen modals: replacing one dismisses it and
// Home shows through before the next is presented.
it('swaps a page of the same kind in place, without re-presenting it', () => {
  const navigation = fakeNavigation({
    name: 'AdapterSession',
    params: {
      target: { protocol: 'rdp', addr: 'win.local' },
      name: 'Windows',
      addrKind: 'lan',
      sessionId: 'adapter-1',
    },
  });

  replaceWithSession(navigation as never, mac);

  expect(navigation.replace).not.toHaveBeenCalled();
  expect(navigation.dispatch).toHaveBeenCalledTimes(1);
  const action = navigation.dispatch.mock.calls[0][0];
  expect(action).toEqual(
    CommonActions.reset({
      ...navigation.getState(),
      routes: [
        { key: 'tabs', name: 'MainTabs' },
        {
          key: 'page',
          name: 'AdapterSession',
          // Whole params: nothing of the Windows route is left behind.
          params: { ...mac.params, sessionId: 'adapter-2', switchSeq: 1 },
        },
      ],
    } as never),
  );
});

it('counts switchSeq up so the page remounts on every swap', () => {
  const navigation = fakeNavigation({
    name: 'Session',
    params: { hostId: 'h0', sessionId: 'host-0', switchSeq: 4 },
  });

  replaceWithSession(navigation as never, host);

  const action = navigation.dispatch.mock.calls[0][0];
  expect(action.payload.routes[1]).toEqual({
    key: 'page',
    name: 'Session',
    params: { hostId: 'h1', sessionId: 'host-1', switchSeq: 5 },
  });
});

it('replaces the page between a Host session and an adapter session', () => {
  const navigation = fakeNavigation({
    name: 'Session',
    params: { hostId: 'h0', sessionId: 'host-0' },
  });

  replaceWithSession(navigation as never, mac);

  expect(navigation.dispatch).not.toHaveBeenCalled();
  expect(navigation.replace).toHaveBeenCalledWith('AdapterSession', {
    ...mac.params,
    sessionId: 'adapter-2',
  });
});
