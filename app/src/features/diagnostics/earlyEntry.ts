import { Platform } from 'react-native';
import { createMmkvEarlyStore } from './earlyStore';
import { installEarlyCrashCapture, type HermesLike } from './earlyCrash';
import type { ErrorUtilsLike } from './jsCrash';

try {
  const g = globalThis as {
    ErrorUtils?: ErrorUtilsLike;
    HermesInternal?: HermesLike;
  };
  installEarlyCrashCapture({
    errorUtils: g.ErrorUtils,
    hermes: g.HermesInternal,
    store: createMmkvEarlyStore(),
    dev: __DEV__,
    osVersion: Platform.Version == null ? null : String(Platform.Version),
  });
} catch {
  // Early crash capture must never prevent the application entry point from loading.
}
