import { Platform } from 'react-native';
import { useEffect, useRef } from 'react';
import { usePowerState } from '../../native/lifecycle/powerStore';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';

/** 把 device.* 运行时提示同步给 Rust（低电量、Android 后台保持）。 */

function useOptionalCore(): RsCoreApi | null {
  try {
    return useCore();
  } catch {
    return null;
  }
}

export default function PowerSync(): null {
  const core = useOptionalCore();
  const lowPower = usePowerState(state => state.power.lowPower);
  const unsupported = useRef(false);
  const backgroundHoldAttempted = useRef<RsCoreApi | null>(null);

  useEffect(() => {
    if (core === null || unsupported.current) {
      return;
    }
    try {
      core.settings.set('device.lowPower', lowPower);
    } catch {
      unsupported.current = true;
      console.warn('device.lowPower sync is unsupported');
    }
  }, [core, lowPower]);

  useEffect(() => {
    if (
      Platform.OS !== 'android' ||
      core === null ||
      backgroundHoldAttempted.current === core
    ) {
      return;
    }
    backgroundHoldAttempted.current = core;
    try {
      core.settings.set('device.backgroundHold', true);
    } catch {
      console.warn('device.backgroundHold sync is unsupported');
    }
  }, [core]);

  return null;
}
