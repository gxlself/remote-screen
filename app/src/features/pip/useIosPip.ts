import { useEffect, useRef } from 'react';
import { Platform } from 'react-native';
import { pip } from '../../i18n/zh/pip';
import { useVideoStats } from '../session/videoStore';
import { useWindowId, windowPhaseNow } from '../session/WindowContext';
import { useCore } from '../../native/rscore/CoreContext';
import {
  pipSupported,
  setPipEligibility,
  subscribePipChange,
  subscribePipPlayback,
} from './nativePip';
import { pipEligibility } from './eligibility';
import { applyPipChange } from './pipState';
import type { SessionController } from '../../screens/Session/useSessionController';

type EligibilitySnapshot = {
  sessionId: string | null;
  canStart: boolean;
  canContinue: boolean;
};

/** iOS 画中画资格同步与事件处理。 */
export function useIosPip(ctx: SessionController): void {
  const core = useCore();
  const windowId = useWindowId();
  const windowRef = useRef(windowId);
  windowRef.current = windowId;
  const stats = useVideoStats(ctx.sessionId);
  const latest = useRef(ctx);
  latest.current = ctx;
  const coreRef = useRef(core);
  coreRef.current = core;
  const previousEligibility = useRef<EligibilitySnapshot | null>(null);
  const enabled = Platform.OS === 'ios' && pipSupported();
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;
  const eligibility = pipEligibility({
    role: ctx.view.role,
    overlay: ctx.view.overlay,
    phase: ctx.rec?.phase,
    width: stats?.width ?? 0,
    height: stats?.height ?? 0,
  });

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const next = {
      sessionId: ctx.sessionId,
      canStart: eligibility.canStart,
      canContinue: eligibility.canContinue,
    };
    const previous = previousEligibility.current;
    if (
      previous !== null &&
      previous.sessionId === next.sessionId &&
      previous.canStart === next.canStart &&
      previous.canContinue === next.canContinue
    ) {
      return;
    }
    previousEligibility.current = next;
    setPipEligibility(next.sessionId, next.canStart, next.canContinue);
  }, [ctx.sessionId, eligibility.canContinue, eligibility.canStart, enabled]);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    return () => {
      previousEligibility.current = null;
      setPipEligibility(null, false, false);
    };
  }, [enabled]);

  useEffect(() => {
    if (!enabledRef.current) {
      return;
    }
    const removePipChange = subscribePipChange(event => {
      applyPipChange(event);
      if (
        !event.active &&
        (event.reason === 'notPossible' || event.reason === 'failed') &&
        // only the window in front says so.
        windowPhaseNow(windowRef.current) === 'active'
      ) {
        latest.current.showToast(pip.notPossible);
      }
    });
    const removePipPlayback = subscribePipPlayback(event => {
      const current = latest.current;
      if (event.sessionId === current.sessionId) {
        coreRef.current.sessions.background(event.sessionId, event.paused);
      }
    });
    return () => {
      removePipChange();
      removePipPlayback();
    };
  }, []);
}
