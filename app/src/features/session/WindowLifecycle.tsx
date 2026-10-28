import { useEffect, useRef } from 'react';
import { useCore } from '../../native/rscore/CoreContext';
import {
  refreshWindows,
  subscribeWindowState,
  windowsSupported,
} from '../../native/window/window';
import { usePipState } from '../pip/pipState';
import type { SwitcherItem } from './switcher';
import {
  anyWindowVisible,
  planWindowPauses,
  setWindowPhase,
  windows,
} from './windows';

/**
 * follows the system windows and pauses only the sessions whose
 * window is off screen (App Exposé, another app full screen) while another
 * window is still up; split view keeps both playing. The whole app going to
 * the background stays SessionLifecycle's. Mounted once, in the main window.
 */
export function WindowLifecycle() {
  const core = useCore();
  const pip = usePipState();
  const pipSessionId = pip.active ? pip.sessionId : null;
  const pipRef = useRef(pipSessionId);
  pipRef.current = pipSessionId;
  const reconcileRef = useRef<() => void>(() => undefined);

  useEffect(() => {
    if (!windowsSupported()) {
      return;
    }
    const remove = subscribeWindowState(event =>
      setWindowPhase(event.windowId, event.state),
    );
    // Events sent before this subscription are lost; ask for them again.
    refreshWindows();
    return remove;
  }, []);

  useEffect(() => {
    if (!windowsSupported()) {
      return;
    }
    const paused = new Map<string, SwitcherItem['kind']>();
    let wasVisible = anyWindowVisible(windows.getState());
    const reconcile = () => {
      const state = windows.getState();
      const visible = anyWindowVisible(state);
      const fresh = visible && !wasVisible;
      wasVisible = visible;
      const plan = planWindowPauses(state, paused, pipRef.current, fresh);
      for (const item of plan.pause) {
        paused.set(item.id, item.kind);
        try {
          core.sessions.background(item.id, true);
        } catch {
          // The session may have ended meanwhile.
        }
      }
      for (const id of plan.resume) {
        paused.delete(id);
        try {
          core.sessions.background(id, false);
        } catch {
          // The session may have ended meanwhile.
        }
      }
      for (const id of plan.drop) {
        paused.delete(id);
      }
    };
    reconcileRef.current = reconcile;
    const unsubscribe = windows.subscribe(reconcile);
    return () => {
      unsubscribe();
      reconcileRef.current = () => undefined;
    };
  }, [core]);

  useEffect(() => {
    reconcileRef.current();
  }, [pipSessionId]);

  return null;
}

export default WindowLifecycle;
