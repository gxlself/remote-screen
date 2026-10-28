import { createMmkvEarlyStore } from './earlyStore';
import {
  EARLY_MAX,
  REJECTIONS_MAX,
  getEarlyCapture,
  installEarlyCrashCapture,
} from './earlyCrash';
import type {
  EarlyStore,
  HermesLike,
  RejectionTrackerOptions,
} from './earlyCrash';
import type { ErrorUtilsLike, GlobalErrorHandler } from './jsCrash';
import type { JsCrashReport } from '../../native/rscore/types';

class MemoryStore implements EarlyStore {
  private value: string | undefined;

  constructor(value?: string) {
    this.value = value;
  }

  read(): string | undefined {
    return this.value;
  }

  write(value: string): void {
    this.value = value;
  }

  clear(): void {
    this.value = undefined;
  }
}

function makeErrorUtils(previous: GlobalErrorHandler = jest.fn()) {
  let current: GlobalErrorHandler | null = previous;
  const errorUtils: ErrorUtilsLike = {
    getGlobalHandler: jest.fn(() => current),
    setGlobalHandler: jest.fn(handler => {
      current = handler;
    }),
  };
  return {
    errorUtils,
    get current(): GlobalErrorHandler | null {
      return current;
    },
  };
}

function reports(store: MemoryStore): Array<Record<string, unknown>> {
  const value = store.read();
  return value === undefined
    ? []
    : (JSON.parse(value) as Array<Record<string, unknown>>);
}

describe('early crash capture', () => {
  afterEach(() => {
    getEarlyCapture()?.uninstall();
  });

  it('captures fatal errors and forwards them to the previous handler', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const store = new MemoryStore();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });

    expect(fake.current).not.toBe(previous);
    const error = new Error('boom');
    fake.current?.(error, true);

    expect(previous).toHaveBeenCalledWith(error, true);
    expect(reports(store)).toEqual([
      expect.objectContaining({ message: 'boom', isFatal: true }),
    ]);
  });

  it('does not cache non-fatal or unspecified global errors', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const store = new MemoryStore();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });
    const error = new Error('ordinary');

    fake.current?.(error, false);
    fake.current?.(error);

    expect(store.read()).toBeUndefined();
    expect(previous).toHaveBeenNthCalledWith(1, error, false);
    expect(previous).toHaveBeenNthCalledWith(2, error, undefined);
  });

  it('hands cached reports to the recorder in order', () => {
    const fake = makeErrorUtils();
    const store = new MemoryStore();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });
    fake.current?.(new Error('e1'), true);
    fake.current?.(new Error('e2'), true);
    const recorder = jest.fn<boolean, [JsCrashReport]>(() => true);

    expect(getEarlyCapture()?.attach(recorder)).toBe(2);
    expect(recorder.mock.calls.map(([report]) => report.message)).toEqual([
      'e1',
      'e2',
    ]);
    expect(store.read()).toBeUndefined();
  });

  it('sends fatal errors directly after attach and caches them after detach', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const store = new MemoryStore();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });
    const recorder = jest.fn<boolean, [JsCrashReport]>(() => true);
    const capture = getEarlyCapture();
    capture?.attach(recorder);

    const attachedError = new Error('attached');
    fake.current?.(attachedError, true);
    expect(recorder).not.toHaveBeenCalled();
    expect(store.read()).toBeUndefined();
    expect(previous).toHaveBeenCalledWith(attachedError, true);

    capture?.detach();
    fake.current?.(new Error('detached'), true);
    expect(reports(store)).toEqual([
      expect.objectContaining({ message: 'detached', isFatal: true }),
    ]);
  });

  it('filters persisted reports and keeps the last five', () => {
    const first = {
      name: null,
      message: 'first',
      stack: null,
      isFatal: true,
      osVersion: null,
    };
    const second = { ...first, message: 'second', isFatal: false };
    const store = new MemoryStore(
      JSON.stringify([first, second, { message: 3, isFatal: true }]),
    );
    const fake = makeErrorUtils();
    const recorder = jest.fn<boolean, [JsCrashReport]>(() => true);
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });

    expect(getEarlyCapture()?.attach(recorder)).toBe(2);
    expect(recorder.mock.calls.map(([report]) => report.message)).toEqual([
      'first',
      'second',
    ]);
  });

  it('clears malformed persisted data', () => {
    const store = new MemoryStore('not json');
    const fake = makeErrorUtils();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });

    expect(getEarlyCapture()?.attach(jest.fn(() => true))).toBe(0);
    expect(store.read()).toBeUndefined();
  });

  it('keeps only the newest five reports', () => {
    const fake = makeErrorUtils();
    const store = new MemoryStore();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });
    for (let i = 0; i < EARLY_MAX + 2; i += 1) {
      fake.current?.(new Error(`e${i}`), true);
    }

    expect(reports(store).map(report => report.message)).toEqual([
      'e2',
      'e3',
      'e4',
      'e5',
      'e6',
    ]);
  });

  it('tracks release promise rejections and caps them at three', () => {
    const tracker = jest.fn();
    const hermes: HermesLike = {
      enablePromiseRejectionTracker: tracker,
    };
    const store = new MemoryStore();
    const fake = makeErrorUtils();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes,
      store,
      dev: false,
      osVersion: null,
    });
    expect(tracker).toHaveBeenCalledTimes(1);
    const options = tracker.mock.calls[0][0] as RejectionTrackerOptions;
    expect(options.allRejections).toBe(true);

    options.onUnhandled(1, new Error('r1'));
    expect(reports(store)).toEqual([
      expect.objectContaining({ message: 'r1', isFatal: false }),
    ]);

    const recorder = jest.fn<boolean, [JsCrashReport]>(() => true);
    getEarlyCapture()?.attach(recorder);
    options.onUnhandled(2, new Error('r2'));
    options.onUnhandled(3, new Error('r3'));
    options.onUnhandled(4, new Error('r4'));
    options.onUnhandled(5, new Error('r5'));
    expect(recorder.mock.calls.map(([report]) => report.message)).toEqual([
      'r1',
      'r2',
      'r3',
    ]);
    expect(recorder).toHaveBeenCalledTimes(REJECTIONS_MAX);
  });

  it('does not install the rejection tracker in development', () => {
    const tracker = jest.fn();
    installEarlyCrashCapture({
      errorUtils: makeErrorUtils().errorUtils,
      hermes: { enablePromiseRejectionTracker: tracker },
      store: new MemoryStore(),
      dev: true,
      osVersion: null,
    });
    expect(tracker).not.toHaveBeenCalled();
  });

  it('handles missing Hermes and ErrorUtils', () => {
    const store = new MemoryStore();
    installEarlyCrashCapture({
      errorUtils: undefined,
      hermes: undefined,
      store,
      dev: false,
      osVersion: null,
    });
    expect(() => getEarlyCapture()?.attach(jest.fn(() => true))).not.toThrow();
  });

  it('swallows recorder and store failures', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const store: EarlyStore = {
      read: () => {
        throw new Error('read');
      },
      write: () => {
        throw new Error('write');
      },
      clear: () => {
        throw new Error('clear');
      },
    };
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store,
      dev: true,
      osVersion: null,
    });
    expect(() => fake.current?.(new Error('boom'), true)).not.toThrow();
    expect(previous).toHaveBeenCalledWith(expect.any(Error), true);
    expect(() =>
      getEarlyCapture()?.attach(() => {
        throw new Error('record');
      }),
    ).not.toThrow();
  });

  it('restores the previous handler on uninstall and ignores late rejections', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const tracker = jest.fn();
    installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: { enablePromiseRejectionTracker: tracker },
      store: new MemoryStore(),
      dev: false,
      osVersion: null,
    });
    const options = tracker.mock.calls[0][0] as RejectionTrackerOptions;
    const capture = getEarlyCapture();
    capture?.uninstall();

    expect(fake.errorUtils.getGlobalHandler()).toBe(previous);
    expect(getEarlyCapture()).toBeNull();
    options.onUnhandled(1, new Error('after'));
    expect(previous).not.toHaveBeenCalled();
    expect(() => capture?.uninstall()).not.toThrow();
  });

  it('leaves an outside global wrapper installed', () => {
    const fake = makeErrorUtils();
    const capture = installEarlyCrashCapture({
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store: new MemoryStore(),
      dev: true,
      osVersion: null,
    });
    const ours = fake.current;
    const wrapper: GlobalErrorHandler = (error, isFatal) => {
      ours?.(error, isFatal);
    };
    fake.errorUtils.setGlobalHandler(wrapper);

    capture.uninstall();
    expect(fake.errorUtils.getGlobalHandler()).toBe(wrapper);
  });

  it('is a singleton until uninstalled', () => {
    const fake = makeErrorUtils();
    const opts = {
      errorUtils: fake.errorUtils,
      hermes: undefined,
      store: new MemoryStore(),
      dev: true,
      osVersion: null,
    };
    const first = installEarlyCrashCapture(opts);
    const second = installEarlyCrashCapture(opts);
    expect(second).toBe(first);
    expect(fake.errorUtils.setGlobalHandler).toHaveBeenCalledTimes(1);
  });

  it('uses the requested MMKV instance and key', () => {
    const store = createMmkvEarlyStore('rs-crash-test');
    expect(store).not.toBeNull();
    store?.write('x');
    expect(store?.read()).toBe('x');
    store?.clear();
    expect(store?.read()).toBeUndefined();
  });
});
