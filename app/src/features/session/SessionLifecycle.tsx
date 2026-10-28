import { useEffect, useRef } from 'react';
import { AppState, Platform } from 'react-native';
import {
  isInPip,
  subscribePipChange,
} from '../../native/androidSession/androidSession';
import {
  isPipActive,
  subscribePipChange as subscribeIosPipChange,
} from '../pip/nativePip';
import { useCore } from '../../native/rscore/CoreContext';
import { windowsSupported } from '../../native/window/window';
import { selectLive, useSessions } from './store';
import { hiddenByWindow, windows } from './windows';

const PIP_EXIT_BACKGROUND_DELAY_MS = 500;

export function SessionLifecycle() {
  const core = useCore();
  const live = useSessions(selectLive);
  const liveRef = useRef(live);
  liveRef.current = live;
  const pipExitTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const backgroundLiveSessions = () => {
      for (const rec of liveRef.current) {
        core.sessions.background(rec.id, true);
      }
    };
    const subscription = AppState.addEventListener('change', state => {
      if (state !== 'background' && state !== 'active') {
        return;
      }
      if (
        state === 'background' &&
        ((Platform.OS === 'android' &&
          (isInPip() || pipExitTimer.current !== null)) ||
          (Platform.OS === 'ios' &&
            (isPipActive() || pipExitTimer.current !== null)))
      ) {
        return;
      }
      // back in front, a session whose own window stays off
      // screen stays paused (WindowLifecycle).
      const byWindow = windowsSupported() ? windows.getState() : null;
      for (const rec of liveRef.current) {
        core.sessions.background(
          rec.id,
          state === 'background' ||
            (byWindow !== null && hiddenByWindow(byWindow, rec.id)),
        );
      }
    });

    let removePipSubscription: () => void = () => undefined;
    if (Platform.OS === 'android') {
      removePipSubscription = subscribePipChange(event => {
        if (pipExitTimer.current !== null) {
          clearTimeout(pipExitTimer.current);
          pipExitTimer.current = null;
        }
        if (event.inPip) {
          return;
        }
        pipExitTimer.current = setTimeout(() => {
          pipExitTimer.current = null;
          if (AppState.currentState === 'active') {
            return;
          }
          backgroundLiveSessions();
        }, PIP_EXIT_BACKGROUND_DELAY_MS);
      });
    } else if (Platform.OS === 'ios') {
      removePipSubscription = subscribeIosPipChange(event => {
        if (pipExitTimer.current !== null) {
          clearTimeout(pipExitTimer.current);
          pipExitTimer.current = null;
        }
        if (event.active) {
          return;
        }
        pipExitTimer.current = setTimeout(() => {
          pipExitTimer.current = null;
          if (AppState.currentState === 'active') {
            return;
          }
          backgroundLiveSessions();
        }, PIP_EXIT_BACKGROUND_DELAY_MS);
      });
    }

    return () => {
      subscription.remove();
      removePipSubscription();
      if (pipExitTimer.current !== null) {
        clearTimeout(pipExitTimer.current);
        pipExitTimer.current = null;
      }
    };
  }, [core]);

  return null;
}

export default SessionLifecycle;
