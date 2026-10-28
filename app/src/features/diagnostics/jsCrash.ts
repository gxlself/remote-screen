import type { JsCrashReport } from '../../native/rscore/types';

export type GlobalErrorHandler = (error: unknown, isFatal?: boolean) => void;

export interface ErrorUtilsLike {
  getGlobalHandler(): GlobalErrorHandler | null | undefined;
  setGlobalHandler(handler: GlobalErrorHandler): void;
}

export const JS_MESSAGE_MAX = 4096;
export const JS_STACK_MAX = 32768;

function truncate(value: string, max: number): string {
  const result = value.slice(0, max);
  if (result.length === 0) {
    return result;
  }
  const last = result.charCodeAt(result.length - 1);
  return last >= 0xd800 && last <= 0xdbff ? result.slice(0, -1) : result;
}

export function toJsCrashReport(
  error: unknown,
  isFatal: boolean,
  osVersion: string | null,
): JsCrashReport {
  try {
    let name: string | null;
    let message: string;
    let stack: string | null;

    if (error instanceof Error) {
      const errorName = error.name;
      name =
        typeof errorName === 'string' && errorName.length > 0
          ? errorName
          : null;
      message = String(error.message);
      const errorStack = error.stack;
      stack = typeof errorStack === 'string' ? errorStack : null;
    } else {
      name = null;
      message = String(error);
      stack = null;
    }

    return {
      name,
      message: truncate(message, JS_MESSAGE_MAX),
      stack: stack === null ? null : truncate(stack, JS_STACK_MAX),
      isFatal,
      osVersion,
    };
  } catch {
    return {
      name: null,
      message: '<unprintable>',
      stack: null,
      isFatal,
      osVersion,
    };
  }
}

export function installJsCrashHandler(opts: {
  errorUtils: ErrorUtilsLike;
  getRecorder: () => ((report: JsCrashReport) => boolean) | null;
  osVersion: string | null;
}): () => void {
  const previous = opts.errorUtils.getGlobalHandler() ?? null;
  let active = true;
  let recording = false;
  let uninstalled = false;

  const handler: GlobalErrorHandler = (error, isFatal) => {
    if (active && isFatal === true && !recording) {
      recording = true;
      try {
        opts.getRecorder()?.(toJsCrashReport(error, true, opts.osVersion));
      } catch {
        // A crash recorder must not interfere with React Native's handler.
      } finally {
        recording = false;
      }
    }

    previous?.(error, isFatal);
  };

  opts.errorUtils.setGlobalHandler(handler);

  return () => {
    if (uninstalled) {
      return;
    }
    uninstalled = true;
    active = false;
    if (previous !== null && opts.errorUtils.getGlobalHandler() === handler) {
      opts.errorUtils.setGlobalHandler(previous);
    }
  };
}
