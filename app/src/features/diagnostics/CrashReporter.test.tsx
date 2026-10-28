import { act, render } from '@testing-library/react-native';
import { MemoryKv } from '../storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import { getEarlyCapture, installEarlyCrashCapture } from './earlyCrash';
import type {
  EarlyStore,
  HermesLike,
  RejectionTrackerOptions,
} from './earlyCrash';
import type { ErrorUtilsLike, GlobalErrorHandler } from './jsCrash';
import CrashReporter from './CrashReporter';

type CrashApi = {
  recordJs: jest.Mock<boolean, [report: unknown]>;
  // Old native builds still expose the Hub upload; nothing may call it.
  uploadPending: jest.Mock;
};

function makeCore() {
  const crash: CrashApi = {
    recordJs: jest.fn<boolean, [unknown]>(() => true),
    uploadPending: jest.fn(),
  };
  return Object.assign(createFakeCore(), { crash });
}

function leftoverUploadKv(): MemoryKv {
  const kv = new MemoryKv();
  kv.setJSON('settings.hub.url', 'http://rs-hub:7300');
  kv.setJSON('settings.crash.upload', true);
  return kv;
}

describe('CrashReporter', () => {
  const globalObject = globalThis as { ErrorUtils?: ErrorUtilsLike };
  let originalErrorUtils: ErrorUtilsLike | undefined;
  let previous: jest.Mock;
  let current: GlobalErrorHandler | null;
  let errorUtils: ErrorUtilsLike;

  beforeEach(() => {
    originalErrorUtils = globalObject.ErrorUtils;
    previous = jest.fn();
    current = previous;
    errorUtils = {
      getGlobalHandler: jest.fn(() => current),
      setGlobalHandler: jest.fn(handler => {
        current = handler;
      }),
    };
    globalObject.ErrorUtils = errorUtils;
  });

  afterEach(() => {
    globalObject.ErrorUtils = originalErrorUtils;
    jest.restoreAllMocks();
  });

  async function flushMicrotasks(): Promise<void> {
    await act(async () => {});
  }

  it('does nothing without providers', async () => {
    await expect(render(<CrashReporter />)).resolves.toBeDefined();
    await flushMicrotasks();
    expect(errorUtils.setGlobalHandler).not.toHaveBeenCalled();
  });

  it('skips the handler when ErrorUtils is absent', async () => {
    delete globalObject.ErrorUtils;
    const core = makeCore();
    await renderWithProviders(<CrashReporter />, { core });
    await flushMicrotasks();

    expect(errorUtils.setGlobalHandler).not.toHaveBeenCalled();
    expect(core.crash.uploadPending).not.toHaveBeenCalled();
  });

  it('installs the handler and ignores a crash upload switch left on', async () => {
    const core = makeCore();
    await renderWithProviders(<CrashReporter />, {
      core,
      kv: leftoverUploadKv(),
    });
    await flushMicrotasks();

    expect(errorUtils.setGlobalHandler).toHaveBeenCalledTimes(1);
    expect(core.crash.uploadPending).not.toHaveBeenCalled();
  });

  it('records fatal errors and forwards them to the previous handler', async () => {
    const core = makeCore();
    await renderWithProviders(<CrashReporter />, { core });
    await flushMicrotasks();
    const error = new Error('x');

    current?.(error, true);
    expect(core.crash.recordJs).toHaveBeenCalledWith(
      expect.objectContaining({ message: 'x', isFatal: true }),
    );
    expect(previous).toHaveBeenCalledWith(error, true);
    expect(core.crash.uploadPending).not.toHaveBeenCalled();
  });

  it('does nothing when the core has no crash API', async () => {
    const core = createFakeCore();
    await renderWithProviders(<CrashReporter />, { core });
    await flushMicrotasks();

    expect(errorUtils.setGlobalHandler).not.toHaveBeenCalled();
  });

  it('restores the previous handler on unmount', async () => {
    const core = makeCore();
    const rendered = await renderWithProviders(<CrashReporter />, { core });
    await act(async () => rendered.unmount());

    expect(errorUtils.getGlobalHandler()).toBe(previous);
  });
});

class EarlyMemoryStore implements EarlyStore {
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

describe('CrashReporter early capture handoff', () => {
  afterEach(() => {
    getEarlyCapture()?.uninstall();
  });

  function installEarly(store: EarlyMemoryStore, hermes: HermesLike) {
    return installEarlyCrashCapture({
      errorUtils: undefined,
      hermes,
      store,
      dev: false,
      osVersion: null,
    });
  }

  it('hands persisted fatal errors and rejections to the crash recorder on mount', async () => {
    const store = new EarlyMemoryStore(
      JSON.stringify([
        {
          name: null,
          message: 'early1',
          stack: null,
          isFatal: true,
          osVersion: null,
        },
        {
          name: null,
          message: 'early2',
          stack: null,
          isFatal: false,
          osVersion: null,
        },
      ]),
    );
    const hermes: HermesLike = {
      enablePromiseRejectionTracker: jest.fn(),
    };
    installEarly(store, hermes);
    const core = makeCore();
    await renderWithProviders(<CrashReporter />, { core });

    expect(core.crash.recordJs.mock.calls.map(([report]) => report)).toEqual([
      expect.objectContaining({ message: 'early1', isFatal: true }),
      expect.objectContaining({ message: 'early2', isFatal: false }),
    ]);
    expect(store.read()).toBeUndefined();
  });

  it('records rejections directly after CrashReporter mounts', async () => {
    const store = new EarlyMemoryStore();
    const tracker = jest.fn();
    installEarly(store, {
      enablePromiseRejectionTracker: tracker,
    });
    const core = makeCore();
    await renderWithProviders(<CrashReporter />, { core });
    const options = tracker.mock.calls[0][0] as RejectionTrackerOptions;

    options.onUnhandled(1, new Error('late'));
    expect(core.crash.recordJs).toHaveBeenCalledWith(
      expect.objectContaining({ message: 'late', isFatal: false }),
    );
  });

  it('returns to persistence after CrashReporter unmounts', async () => {
    const store = new EarlyMemoryStore();
    const tracker = jest.fn();
    installEarly(store, {
      enablePromiseRejectionTracker: tracker,
    });
    const core = makeCore();
    const rendered = await renderWithProviders(<CrashReporter />, { core });
    const options = tracker.mock.calls[0][0] as RejectionTrackerOptions;
    await act(async () => rendered.unmount());

    options.onUnhandled(2, new Error('after'));
    expect(core.crash.recordJs).not.toHaveBeenCalled();
    expect(JSON.parse(store.read() ?? '[]')).toEqual([
      expect.objectContaining({ message: 'after', isFatal: false }),
    ]);
  });

  it('keeps the cache when the core has no crash API', async () => {
    const cached = {
      name: null,
      message: 'waiting',
      stack: null,
      isFatal: true,
      osVersion: null,
    };
    const store = new EarlyMemoryStore(JSON.stringify([cached]));
    const hermes: HermesLike = {
      enablePromiseRejectionTracker: jest.fn(),
    };
    installEarly(store, hermes);
    await renderWithProviders(<CrashReporter />, { core: createFakeCore() });

    expect(JSON.parse(store.read() ?? '[]')).toEqual([cached]);
  });
});
