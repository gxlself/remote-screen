import type { JsCrashReport } from '../../native/rscore/types';
import {
  toJsCrashReport,
  type ErrorUtilsLike,
  type GlobalErrorHandler,
} from './jsCrash';

export const EARLY_KEY = 'crash.early.v1';
export const EARLY_MAX = 5;
export const REJECTIONS_MAX = 3;

export interface EarlyStore {
  read(): string | undefined;
  write(value: string): void;
  clear(): void;
}

export type RejectionTrackerOptions = {
  allRejections: boolean;
  onUnhandled: (id: number, error: unknown) => void;
  onHandled: (id: number) => void;
};

export interface HermesLike {
  enablePromiseRejectionTracker?: (options: RejectionTrackerOptions) => void;
}

export type Recorder = (report: JsCrashReport) => boolean;

export interface EarlyCapture {
  /** CrashReporter 就绪：按先后交出全部缓存并清空持久化，返回交出的条数。 */
  attach(recorder: Recorder): number;
  /** CrashReporter 卸载：回到缓存模式。 */
  detach(): void;
  /** 测试用：恢复原处理器、停用 tracker 回调、清掉单例。 */
  uninstall(): void;
}

let installed: EarlyCapture | null = null;

function isJsCrashReport(value: unknown): value is JsCrashReport {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }

  const report = value as Record<string, unknown>;
  return (
    typeof report.message === 'string' &&
    typeof report.isFatal === 'boolean' &&
    (typeof report.name === 'string' || report.name === null) &&
    (typeof report.stack === 'string' || report.stack === null) &&
    (typeof report.osVersion === 'string' || report.osVersion === null)
  );
}

function readReports(store: EarlyStore | null): JsCrashReport[] {
  if (store === null) {
    return [];
  }

  let raw: string | undefined;
  try {
    raw = store.read();
  } catch {
    return [];
  }

  if (raw === undefined) {
    return [];
  }

  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      throw new Error('early crash cache is not an array');
    }
    const valid = parsed.filter(isJsCrashReport);
    return valid.slice(-EARLY_MAX);
  } catch {
    try {
      store.clear();
    } catch {
      // Persistence is best effort during early startup.
    }
    return [];
  }
}

export function installEarlyCrashCapture(opts: {
  errorUtils: ErrorUtilsLike | undefined;
  hermes: HermesLike | undefined;
  store: EarlyStore | null;
  dev: boolean;
  osVersion: string | null;
}): EarlyCapture {
  if (installed !== null) {
    return installed;
  }

  const store = opts.store;
  const cached = readReports(store);
  const previous = opts.errorUtils?.getGlobalHandler() ?? null;
  let attached = false;
  let recorder: Recorder | null = null;
  let uninstalled = false;
  let rejectionCount = 0;

  function push(report: JsCrashReport): void {
    cached.push(report);
    if (cached.length > EARLY_MAX) {
      cached.splice(0, cached.length - EARLY_MAX);
    }
    try {
      store?.write(JSON.stringify(cached));
    } catch {
      // MMKV is best effort. The in-memory copy is still available to attach.
    }
  }

  const handler: GlobalErrorHandler = (error, isFatal) => {
    if (!uninstalled && !attached && isFatal === true) {
      try {
        push(toJsCrashReport(error, true, opts.osVersion));
      } catch {
        // Early capture must never interfere with React Native's handler.
      }
    }

    // Preserve React Native's handler semantics, including errors it throws.
    previous?.(error, isFatal);
  };

  if (opts.errorUtils !== undefined) {
    opts.errorUtils.setGlobalHandler(handler);
  }

  const capture: EarlyCapture = {
    attach(nextRecorder) {
      if (uninstalled) {
        return 0;
      }

      attached = true;
      recorder = nextRecorder;
      const reports = cached.slice();
      for (const report of reports) {
        try {
          nextRecorder(report);
        } catch {
          // A recorder failure must not prevent the remaining reports from being delivered.
        }
      }
      cached.length = 0;
      try {
        store?.clear();
      } catch {
        // Persistence is best effort during handoff.
      }
      return reports.length;
    },

    detach() {
      attached = false;
      recorder = null;
    },

    uninstall() {
      if (uninstalled) {
        return;
      }
      uninstalled = true;
      attached = false;
      recorder = null;

      if (opts.errorUtils !== undefined && previous !== null) {
        try {
          if (opts.errorUtils.getGlobalHandler() === handler) {
            opts.errorUtils.setGlobalHandler(previous);
          }
        } catch {
          // Restoring a handler is best effort during teardown.
        }
      }
      if (installed === capture) {
        installed = null;
      }
    },
  };

  if (
    !opts.dev &&
    typeof opts.hermes?.enablePromiseRejectionTracker === 'function'
  ) {
    opts.hermes.enablePromiseRejectionTracker({
      allRejections: true,
      onUnhandled: (_id, error) => {
        if (uninstalled || rejectionCount >= REJECTIONS_MAX) {
          return;
        }
        rejectionCount += 1;
        const report = toJsCrashReport(error, false, opts.osVersion);
        if (attached) {
          try {
            recorder?.(report);
          } catch {
            // A recorder failure must not interfere with the rejection tracker.
          }
        } else {
          push(report);
        }
      },
      onHandled: () => {},
    });
  }

  installed = capture;
  return capture;
}

export function getEarlyCapture(): EarlyCapture | null {
  return installed;
}
