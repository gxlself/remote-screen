import {
  JS_MESSAGE_MAX,
  JS_STACK_MAX,
  installJsCrashHandler,
  toJsCrashReport,
} from './jsCrash';
import type { ErrorUtilsLike, GlobalErrorHandler } from './jsCrash';

function makeErrorUtils(previous: GlobalErrorHandler | null | undefined) {
  let current = previous;
  const errorUtils: ErrorUtilsLike = {
    getGlobalHandler: jest.fn(() => current),
    setGlobalHandler: jest.fn(handler => {
      current = handler;
    }),
  };
  return {
    errorUtils,
    get current() {
      return current;
    },
  };
}

describe('toJsCrashReport', () => {
  it('extracts Error details', () => {
    const report = toJsCrashReport(new TypeError('bad'), true, '17.4');

    expect(report.name).toBe('TypeError');
    expect(report.message).toBe('bad');
    expect(report.stack).toEqual(expect.stringContaining('bad'));
    expect(report.isFatal).toBe(true);
    expect(report.osVersion).toBe('17.4');
  });

  it('uses null for an empty Error name', () => {
    const error = new Error('x');
    error.name = '';
    expect(toJsCrashReport(error, true, null).name).toBeNull();
  });

  it.each([
    ['str', 'str'],
    [42, '42'],
    [null, 'null'],
    [undefined, 'undefined'],
  ])('prints non-Error value %p', (value, message) => {
    expect(toJsCrashReport(value, true, null)).toEqual({
      name: null,
      message,
      stack: null,
      isFatal: true,
      osVersion: null,
    });
  });

  it('uses a safe fallback when an error cannot be printed', () => {
    const unprintable = {
      toString: () => {
        throw new Error('no');
      },
    };
    const error = new Error('x');
    Object.defineProperty(error, 'message', {
      configurable: true,
      get() {
        throw new Error('no');
      },
    });

    expect(toJsCrashReport(unprintable, true, null).message).toBe(
      '<unprintable>',
    );
    expect(toJsCrashReport(error, true, null)).toMatchObject({
      name: null,
      message: '<unprintable>',
      stack: null,
    });
  });

  it('falls back when inspecting a Proxy throws', () => {
    const error = new Proxy(
      {},
      {
        getPrototypeOf() {
          throw new Error('no');
        },
      },
    );

    expect(toJsCrashReport(error, false, '17.4')).toEqual({
      name: null,
      message: '<unprintable>',
      stack: null,
      isFatal: false,
      osVersion: '17.4',
    });
  });

  it('truncates messages and stacks without leaving a high surrogate', () => {
    const longMessage = toJsCrashReport('a'.repeat(5000), true, null);
    expect(longMessage.message.length).toBe(JS_MESSAGE_MAX);

    const error = new Error('x');
    Object.defineProperty(error, 'stack', {
      configurable: true,
      value: 'b'.repeat(40000),
    });
    const longStack = toJsCrashReport(error, true, null);
    expect(longStack.stack?.length).toBe(JS_STACK_MAX);

    const emoji = toJsCrashReport('a'.repeat(4095) + '😀', true, null);
    expect(emoji.message.length).toBe(4095);
    expect(emoji.message.charCodeAt(emoji.message.length - 1)).not.toBe(0xd83d);
    expect(JSON.stringify(emoji)).not.toContain('\\ud83d');

    Object.defineProperty(error, 'stack', {
      value: 'b'.repeat(JS_STACK_MAX - 1) + '😀',
    });
    const emojiStack = toJsCrashReport(error, true, null);
    expect(emojiStack.stack?.length).toBe(JS_STACK_MAX - 1);
    expect(JSON.stringify(emojiStack)).not.toContain('\\ud83d');
  });
});

describe('installJsCrashHandler', () => {
  it('records fatal errors and forwards every call to the previous handler', () => {
    const previous = jest.fn();
    const recorder = jest.fn<boolean, [unknown]>(() => true);
    const fake = makeErrorUtils(previous);
    const uninstall = installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => recorder,
      osVersion: '17.4',
    });
    const error = new Error('boom');

    fake.current?.(error, true);
    expect(recorder).toHaveBeenCalledTimes(1);
    expect(recorder.mock.calls[0][0]).toMatchObject({
      message: 'boom',
      isFatal: true,
    });
    expect(previous).toHaveBeenCalledWith(error, true);
    expect(previous).toHaveBeenCalledTimes(1);
    expect(recorder.mock.invocationCallOrder[0]).toBeLessThan(
      previous.mock.invocationCallOrder[0],
    );

    fake.current?.(error, false);
    fake.current?.(error);
    expect(recorder).toHaveBeenCalledTimes(1);
    expect(previous).toHaveBeenNthCalledWith(2, error, false);
    expect(previous).toHaveBeenNthCalledWith(3, error, undefined);

    uninstall();
  });

  it('handles a missing recorder and recorder lookup failures', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const current = () => fake.current?.(new Error('x'), true);

    installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => null,
      osVersion: null,
    });
    expect(current).not.toThrow();

    const failing = makeErrorUtils(previous);
    expect(() =>
      installJsCrashHandler({
        errorUtils: failing.errorUtils,
        getRecorder: () => {
          throw new Error('recorder');
        },
        osVersion: null,
      }),
    ).not.toThrow();
    expect(failing.current).not.toBeNull();
    failing.current?.(new Error('x'), true);
    expect(previous).toHaveBeenCalledTimes(2);
  });

  it.each(['throw', 'reject'] as const)(
    'forwards errors when the recorder result is %s',
    outcome => {
      const previous = jest.fn();
      const fake = makeErrorUtils(previous);
      const recorder = jest.fn(() => {
        if (outcome === 'throw') {
          throw new Error('recorder');
        }
        return false;
      });
      installJsCrashHandler({
        errorUtils: fake.errorUtils,
        getRecorder: () => recorder,
        osVersion: null,
      });
      const error = new Error('boom');

      expect(() => fake.current?.(error, true)).not.toThrow();
      expect(recorder).toHaveBeenCalledTimes(1);
      expect(previous).toHaveBeenCalledTimes(1);
      expect(previous).toHaveBeenCalledWith(error, true);
    },
  );

  it('preserves errors thrown by the previous handler', () => {
    const failure = new Error('previous');
    const previous = jest.fn(() => {
      throw failure;
    });
    const fake = makeErrorUtils(previous);
    installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => null,
      osVersion: null,
    });

    expect(() => fake.current?.(new Error('boom'), true)).toThrow(failure);
    expect(previous).toHaveBeenCalledTimes(1);
  });

  it('prevents reentrant recording', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    let current: GlobalErrorHandler | undefined;
    const recorder = jest.fn(() => {
      current?.(new Error('inner'), true);
      return true;
    });
    installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => recorder,
      osVersion: null,
    });
    current = fake.current ?? undefined;

    current?.(new Error('outer'), true);
    expect(recorder).toHaveBeenCalledTimes(1);
  });

  it('supports an undefined previous handler', () => {
    const fake = makeErrorUtils(undefined);
    const recorder = jest.fn(() => true);
    const uninstall = installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => recorder,
      osVersion: null,
    });
    expect(() => fake.current?.(new Error('x'), true)).not.toThrow();
    expect(recorder).toHaveBeenCalledTimes(1);
    const handler = fake.current;
    uninstall();
    expect(fake.current).toBe(handler);
    fake.current?.(new Error('inactive'), true);
    expect(recorder).toHaveBeenCalledTimes(1);
  });

  it('disables a saved handler on uninstall and restores the previous one', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const recorder = jest.fn(() => true);
    const uninstall = installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => recorder,
      osVersion: null,
    });
    const oldHandler = fake.current;

    uninstall();
    expect(fake.current).toBe(previous);
    oldHandler?.(new Error('old'), true);
    expect(recorder).not.toHaveBeenCalled();
    expect(previous).toHaveBeenCalledWith(expect.any(Error), true);
    expect(() => uninstall()).not.toThrow();
  });

  it('leaves an outside wrapper installed', () => {
    const previous = jest.fn();
    const fake = makeErrorUtils(previous);
    const recorder = jest.fn(() => true);
    const uninstall = installJsCrashHandler({
      errorUtils: fake.errorUtils,
      getRecorder: () => recorder,
      osVersion: null,
    });
    const ours = fake.current;
    const wrapper: GlobalErrorHandler = (error, isFatal) => {
      ours?.(error, isFatal);
    };
    fake.errorUtils.setGlobalHandler(wrapper);

    uninstall();
    expect(fake.current).toBe(wrapper);
    wrapper(new Error('wrapped'), true);
    expect(recorder).not.toHaveBeenCalled();
    expect(previous).toHaveBeenCalledTimes(1);
  });
});
