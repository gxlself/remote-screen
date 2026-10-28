import { useCallback, useEffect, useMemo, useRef } from 'react';
import { Platform } from 'react-native';
import { navigationRef } from '../../app/navigation/navigationRef';
import { useLockState } from '../../features/lock/lockState';
import { selectLive, useSessions } from '../../features/session/store';
import { useSetting, useSettingsStore } from '../../features/settings/store';
import { useCore } from '../rscore/CoreContext';
import {
  setActiveSessions,
  setTileTarget,
  subscribeNotificationAction,
  subscribeTileConnect,
  takePendingTileConnect,
} from './androidSession';
import {
  routeForNotification,
  routeForTile,
  type SessionTarget,
} from './tileRoute';

export default function AndroidSessionBridge(): null | React.JSX.Element {
  if (Platform.OS !== 'android') {
    return null;
  }
  return <AndroidSessionBridgeAndroid />;
}

function AndroidSessionBridgeAndroid(): null {
  const core = useCore();
  const live = useSessions(selectLive);
  const liveRef = useRef(live);
  liveRef.current = live;
  const locked = useLockState(state => state.locked);
  const [onboardingDone] = useSetting('onboarding.done');
  const settingsStore = useSettingsStore();
  const pendingNavigation = useRef<SessionTarget | null>(null);
  const activeSessions = useMemo(
    () =>
      live.map(session => ({
        sessionId: session.id,
        title: session.hostName,
        kind: 'desktop',
        startedAtMs: Math.floor(session.startedAt / 1000),
      })),
    [live],
  );
  const activeSessionsKey = JSON.stringify(activeSessions);
  const mostRecent = live[live.length - 1];
  const mostRecentKey = mostRecent
    ? `${mostRecent.id}:${mostRecent.hostId}:${mostRecent.hostName}:${mostRecent.startedAt}`
    : '';
  const sentActiveSessionsKey = useRef<string | null>(null);
  const sentTileTargetKey = useRef<string | null>(null);

  const navigatePending = useCallback(() => {
    const target = pendingNavigation.current;
    if (target === null) {
      return;
    }
    // AppLockGate publishes its launch decision in an effect. Read the stores
    // again so an event cannot use a stale unlocked/onboarding render.
    if (!settingsStore.get('onboarding.done')) {
      pendingNavigation.current = null;
      return;
    }
    if (useLockState.getState().locked || !navigationRef.isReady()) {
      return;
    }
    pendingNavigation.current = null;
    const currentRoute = navigationRef.getCurrentRoute();
    if (
      target.sessionId !== undefined &&
      currentRoute?.name === 'Session' &&
      currentRoute.params?.sessionId === target.sessionId
    ) {
      return;
    }
    navigationRef.navigate('Session', target);
  }, [settingsStore]);

  const requestNavigation = useCallback(
    (target: SessionTarget | null) => {
      if (target !== null) {
        pendingNavigation.current = target;
        navigatePending();
      }
    },
    [navigatePending],
  );

  useEffect(() => {
    if (sentActiveSessionsKey.current === activeSessionsKey) {
      return;
    }
    sentActiveSessionsKey.current = activeSessionsKey;
    setActiveSessions(activeSessions);
  }, [activeSessions, activeSessionsKey]);

  useEffect(() => {
    if (
      mostRecent === undefined ||
      sentTileTargetKey.current === mostRecentKey
    ) {
      return;
    }
    sentTileTargetKey.current = mostRecentKey;
    setTileTarget(mostRecent.hostId, mostRecent.hostName);
  }, [mostRecent, mostRecentKey]);

  useEffect(
    () => () => {
      setActiveSessions([]);
    },
    [],
  );

  useEffect(() => {
    const unsubscribeState = navigationRef.addListener(
      'state',
      navigatePending,
    );
    const unsubscribeReady = navigationRef.addListener(
      'ready',
      navigatePending,
    );
    return () => {
      pendingNavigation.current = null;
      unsubscribeState();
      unsubscribeReady();
    };
  }, [navigatePending]);

  useEffect(() => {
    navigatePending();
  }, [locked, onboardingDone, navigatePending]);

  useEffect(() => {
    const receiveTileRequest = (hostId: string) => {
      requestNavigation(routeForTile(hostId, liveRef.current));
    };
    const unsubscribe = subscribeTileConnect(event => {
      receiveTileRequest(event.hostId);
    });
    const pendingHostId = takePendingTileConnect();
    if (pendingHostId !== '') {
      receiveTileRequest(pendingHostId);
    }
    return unsubscribe;
  }, [requestNavigation]);

  useEffect(() => {
    return subscribeNotificationAction(event => {
      if (event.action === 'disconnect') {
        core.sessions.close(event.sessionId);
      } else if (event.action === 'open') {
        requestNavigation(
          routeForNotification(event.sessionId, liveRef.current),
        );
      }
    });
  }, [core, requestNavigation]);

  return null;
}
