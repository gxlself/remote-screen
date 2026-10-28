import { useCallback, useEffect, useRef, useState } from 'react';

/** 这么久没碰屏幕，工具栏就淡出，让出 Dock 和开始按钮。 */
export const TOOLBAR_IDLE_MS = 6000;

/**
 * 工具栏空闲淡出（R24）。enabled 为 false（面板、键盘、读屏开着等）时一直显示；
 * 任何触摸调用 wake() 恢复并重新计时。
 */
export function useToolbarIdle(enabled: boolean): {
  faded: boolean;
  wake: () => void;
} {
  const [faded, setFaded] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;

  const clear = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const wake = useCallback(() => {
    clear();
    setFaded(false);
    if (!enabledRef.current) {
      return;
    }
    timer.current = setTimeout(() => {
      timer.current = null;
      setFaded(true);
    }, TOOLBAR_IDLE_MS);
  }, [clear]);

  useEffect(() => {
    wake();
    return clear;
  }, [clear, enabled, wake]);

  return { faded: enabled && faded, wake };
}
