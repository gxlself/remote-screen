import { useMemo } from 'react';
import { CommonActions } from '@react-navigation/native';
import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import type { RootStackParamList } from '../../app/navigation/routes';
import { selectCollapsed, useAdapterLive } from '../adapters/liveStore';
import { useDirectory, useSshServers } from '../data/queries';
import { selectLiveSsh, useSshSessions } from '../sessions/sshStore';
import { windowsSupported } from '../../native/window/window';
import { markArrival } from './spaces';
import { selectLive, useSessions } from './store';
import {
  adapterSwitcherItem,
  buildSwitcherList,
  hostSwitcherItem,
  sshSwitcherItem,
} from './switcher';
import type { SwitcherItem, SwitcherList } from './switcher';
import { useWindowId } from './WindowContext';
import { shownInOtherWindows, useWindows } from './windows';

/** Every open session, oldest first; `current` is the page's own one. */
export function useSwitcherList(current: SwitcherItem | null): SwitcherList {
  const { directory } = useDirectory();
  const serversQuery = useSshServers();
  const remote = useSessions(selectLive);
  const collapsed = useAdapterLive(selectCollapsed);
  const ssh = useSshSessions(selectLiveSsh);
  const windowId = useWindowId();
  // an adapter session on screen in another window is in no store
  // this page reads; that window's page says it shows it.
  const elsewhere = useWindows(state =>
    windowsSupported() ? shownInOtherWindows(state, windowId) : [],
  );
  return useMemo(() => {
    const hostNames = new Map(
      (directory?.computers ?? []).map(computer => [
        computer.hostId,
        computer.name,
      ]),
    );
    const serverNames = new Map(
      (serversQuery.data ?? []).map(server => [server.serverId, server.name]),
    );
    return buildSwitcherList(
      [
        ...elsewhere,
        ...remote.map(record =>
          hostSwitcherItem(
            record,
            hostNames.get(record.hostId) ?? record.hostName,
          ),
        ),
        ...collapsed.map(adapterSwitcherItem),
        ...ssh.map(record =>
          sshSwitcherItem(
            record,
            serverNames.get(record.serverId) ?? record.serverId,
          ),
        ),
      ],
      current,
    );
  }, [
    collapsed,
    current,
    directory,
    elsewhere,
    remote,
    serversQuery.data,
    ssh,
  ]);
}

/** ids of the sessions on screen in another window. */
export function useElsewhereIds(): string[] {
  const windowId = useWindowId();
  return useWindows(state =>
    windowsSupported()
      ? shownInOtherWindows(state, windowId).map(item => item.id)
      : [],
  );
}

/**
 * The swipe and ⌃⌥⌘←/→ order skips sessions shown in another window: they
 * are not taken over, their window is.
 */
export function withoutElsewhere(
  list: SwitcherList,
  elsewhereIds: readonly string[],
): SwitcherList {
  if (elsewhereIds.length === 0) {
    return list;
  }
  return {
    screens: list.screens.filter(item => !elsewhereIds.includes(item.id)),
    terminals: list.terminals,
  };
}

/**
 * Opens `item` in place of the current page: replace, so the stack does not
 * grow and back still returns to the page before the session.
 */
export function replaceWithSession<R extends keyof RootStackParamList>(
  navigation: NativeStackNavigationProp<RootStackParamList, R>,
  item: SwitcherItem,
): void {
  if (item.kind !== 'ssh') {
    // The page opening it shows its last picture and the position dots.
    markArrival(item.id);
  }
  switch (item.kind) {
    case 'host':
      openScreen(navigation, 'Session', {
        hostId: item.hostId,
        sessionId: item.id,
      });
      return;
    case 'vnc':
    case 'rdp':
      openScreen(navigation, 'AdapterSession', {
        ...item.params,
        sessionId: item.id,
      });
      return;
    case 'ssh':
      navigation.replace('Terminal', {
        serverId: item.serverId,
        sshId: item.id,
      });
      return;
  }
}

/**
 * The session pages are full-screen modals: replace dismisses one and
 * presents the next, and Home shows through in between. Between two pages
 * of the same kind the route keeps its key and only its params change; the
 * page remounts on `switchSeq`.
 */
function openScreen<
  R extends keyof RootStackParamList,
  S extends 'Session' | 'AdapterSession',
>(
  navigation: NativeStackNavigationProp<RootStackParamList, R>,
  name: S,
  params: NonNullable<RootStackParamList[S]>,
): void {
  // Test doubles may have no state.
  const state =
    typeof navigation.getState === 'function'
      ? navigation.getState()
      : undefined;
  const top = state?.routes[state.index];
  if (state === undefined || top === undefined || top.name !== name) {
    (navigation.replace as (n: S, p: RootStackParamList[S]) => void)(
      name,
      params,
    );
    return;
  }
  const previous = (top.params as { switchSeq?: number } | undefined)
    ?.switchSeq;
  const routes = state.routes.map((route, index) =>
    index === state.index
      ? { ...route, params: { ...params, switchSeq: (previous ?? 0) + 1 } }
      : route,
  );
  navigation.dispatch(CommonActions.reset({ ...state, routes }));
}

/**
 * 连接其他电脑: back down to Home's computer list; a computer opened there
 * goes full screen. popTo, not navigate: React Navigation 7's navigate pushes
 * a second MainTabs over the session, which then never unmounts, so it is
 * never folded away or paused and the stack keeps growing.
 */
export function openComputerPicker<R extends keyof RootStackParamList>(
  navigation: NativeStackNavigationProp<RootStackParamList, R>,
): void {
  navigation.popTo('MainTabs', { screen: 'Home' });
}
