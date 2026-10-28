import { useEffect } from 'react';
import { useLiveSessionCount } from '../../features/session/useLiveSessionCount';
import { useCore } from '../rscore/CoreContext';
import {
  getPowerState,
  setBackgroundKeepAlive,
  setIdleTimerDisabled,
  subscribePowerChange,
} from './lifecycle';
import { usePowerState } from './powerStore';

export default function LifecycleBridge(): null {
  const core = useCore();
  const live = useLiveSessionCount() > 0;
  const power = usePowerState(state => state.power);
  const setPower = usePowerState(state => state.setPower);

  useEffect(() => {
    setIdleTimerDisabled(live);
    setBackgroundKeepAlive(live);
  }, [live]);

  useEffect(
    () => () => {
      setIdleTimerDisabled(false);
      setBackgroundKeepAlive(false);
    },
    [],
  );

  useEffect(() => {
    let mounted = true;
    const unsubscribe = subscribePowerChange(next => {
      if (mounted) {
        setPower(next);
      }
    });
    getPowerState()
      .then(next => {
        if (mounted) {
          setPower(next);
        }
      })
      .catch(() => {
        // The lifecycle module is optional on Android and in Jest.
      });
    return () => {
      mounted = false;
      unsubscribe();
    };
  }, [setPower]);

  useEffect(() => {
    core.logs.write('info', 'lifecycle', `lowPower=${String(power.lowPower)}`);
  }, [core, power.lowPower]);

  return null;
}
