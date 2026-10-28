import { useEffect, useRef } from 'react';
import { BackHandler, Platform } from 'react-native';
import { androidSession } from '../../i18n/zh/androidSession';
import type { SessionController } from './useSessionController';

const BACK_WINDOW_MS = 3000;

export function useAndroidBackPress(ctx: SessionController): void {
  const latest = useRef(ctx);
  const lastBackAt = useRef<number | null>(null);
  latest.current = ctx;

  useEffect(() => {
    lastBackAt.current = null;
  }, [ctx.sessionId]);

  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }

    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        const current = latest.current;
        if (current.sheet !== null) {
          current.closeSheet();
          return true;
        }
        if (current.keyboardOpen) {
          current.closeKeyboard();
          return true;
        }
        if (current.coachVisible) {
          current.closeCoach();
          return true;
        }
        const overlay = current.view.overlay;
        if (
          overlay === 'opening' ||
          overlay === 'connecting' ||
          overlay === 'approval' ||
          (overlay === 'none' &&
            current.rec?.phase === 'streaming' &&
            current.firstFrameAtMs === null)
        ) {
          // 还没连上就返回等于取消：关掉这条连接，不留在后台。
          current.cancelConnect();
          return true;
        }
        if (overlay !== 'none') {
          return false;
        }

        const now = Date.now();
        if (
          lastBackAt.current !== null &&
          now - lastBackAt.current <= BACK_WINDOW_MS
        ) {
          lastBackAt.current = null;
          current.collapse();
          return true;
        }

        // 第一次返回只提示，不再顺手收起工具栏。
        current.showToast(androidSession.backAgain);
        lastBackAt.current = now;
        return true;
      },
    );

    return () => {
      subscription.remove();
      lastBackAt.current = null;
    };
  }, []);
}
