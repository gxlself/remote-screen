import { useCallback, useEffect, useRef, useState } from 'react';
import type { HudModel } from './hud';

export type HudExpansion = 'expanded' | 'collapsed' | 'alert';

const EXPANDED_MS = 3000;
const ALERT_MS = 5000;

export function useHudExpansion(model: HudModel): HudExpansion {
  const [expansion, setExpansion] = useState<HudExpansion>('expanded');
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const previous = useRef({
    path: model.path,
    tcp: model.tcp,
    alertText: model.alertText,
  });

  const clearTimer = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  const schedule = useCallback(
    (next: HudExpansion, delay: number) => {
      clearTimer();
      setExpansion(next);
      timer.current = setTimeout(() => {
        timer.current = null;
        setExpansion('collapsed');
      }, delay);
    },
    [clearTimer],
  );

  useEffect(() => {
    schedule('expanded', EXPANDED_MS);
    return clearTimer;
  }, [clearTimer, schedule]);

  useEffect(() => {
    const before = previous.current;
    previous.current = {
      path: model.path,
      tcp: model.tcp,
      alertText: model.alertText,
    };

    const alertAppeared = before.alertText === null && model.alertText !== null;
    if (alertAppeared) {
      schedule('alert', ALERT_MS);
      return;
    }

    if (before.path !== model.path || before.tcp !== model.tcp) {
      schedule('expanded', EXPANDED_MS);
    }
  }, [model.path, model.tcp, model.alertText, schedule]);

  return expansion;
}
