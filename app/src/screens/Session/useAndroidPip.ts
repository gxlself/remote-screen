import { useEffect, useRef } from 'react';
import { Platform } from 'react-native';
import {
  setPipEligible,
  subscribePipChange,
} from '../../native/androidSession/androidSession';
import { useVideoStats } from '../../features/session/videoStore';
import type { SessionController } from './useSessionController';

export function useAndroidPip(ctx: SessionController): void {
  const stats = useVideoStats(ctx.sessionId);
  const latest = useRef(ctx);
  latest.current = ctx;
  const inPip = useRef(false);
  const toolbarHiddenByHook = useRef(false);

  const width = stats?.width ?? 0;
  const height = stats?.height ?? 0;
  const eligible =
    ctx.view.role === 'viewer' &&
    ctx.view.overlay === 'none' &&
    ctx.rec?.phase === 'streaming' &&
    stats !== null &&
    stats.width > 0 &&
    stats.height > 0;

  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }
    setPipEligible(eligible, width, height);
  }, [eligible, height, width]);

  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }
    return () => {
      setPipEligible(false, 0, 0);
    };
  }, []);

  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }
    return subscribePipChange(event => {
      const current = latest.current;
      if (event.inPip) {
        if (inPip.current) {
          return;
        }
        inPip.current = true;
        if (current.sheet !== null) {
          current.closeSheet();
        }
        if (current.keyboardOpen) {
          current.closeKeyboard();
        }
        if (!current.toolbarHidden) {
          current.onGesture({ kind: 'threeFingerTap' });
          toolbarHiddenByHook.current = true;
        }
        return;
      }

      if (!inPip.current) {
        return;
      }
      inPip.current = false;
      if (toolbarHiddenByHook.current) {
        current.onGesture({ kind: 'threeFingerTap' });
        toolbarHiddenByHook.current = false;
      }
    });
  }, []);
}
