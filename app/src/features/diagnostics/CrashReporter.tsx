import { Platform } from 'react-native';
import { useEffect } from 'react';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { installJsCrashHandler, type ErrorUtilsLike } from './jsCrash';
import { getEarlyCapture } from './earlyCrash';

function useOptionalCore(): RsCoreApi | null {
  try {
    return useCore();
  } catch {
    return null;
  }
}

export default function CrashReporter(): null {
  const crash = useOptionalCore()?.crash;

  useEffect(() => {
    if (crash === undefined) {
      return;
    }
    const errorUtils = (globalThis as { ErrorUtils?: ErrorUtilsLike })
      .ErrorUtils;
    const osVersion =
      Platform.Version == null ? null : String(Platform.Version);
    const early = getEarlyCapture();
    const uninstall =
      errorUtils == null
        ? undefined
        : installJsCrashHandler({
            errorUtils,
            getRecorder: () => report => crash.recordJs(report),
            osVersion,
          });
    early?.attach(report => crash.recordJs(report));
    return () => {
      early?.detach();
      uninstall?.();
    };
  }, [crash]);

  return null;
}
