import { act, cleanup, renderHook } from '@testing-library/react-native';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type { AdapterErrorInfo } from '../../native/rscore/types';
import { LOST_IN_BACKGROUND, type AdapterUiPhase } from './logic';
import { useAdapterReconnect } from './useAdapterReconnect';

type Props = { phase: AdapterUiPhase; error: AdapterErrorInfo | null };

const DROP: AdapterErrorInfo = {
  code: 'timeout',
  rsCode: ADAPTER_RS_E.unreachable,
  message: 'link lost',
};
const STREAMING: Props = { phase: 'streaming', error: null };
const DROPPED: Props = { phase: 'ended', error: DROP };

async function setup(initial: Props = STREAMING) {
  let appState: ((state: AppStateStatus) => void) | null = null;
  jest
    .spyOn(AppState, 'addEventListener')
    .mockImplementation((_type, listener) => {
      appState = listener as (state: AppStateStatus) => void;
      return { remove: jest.fn() } as unknown as ReturnType<
        typeof AppState.addEventListener
      >;
    });
  const retry = jest.fn();
  const end = jest.fn();
  const isAlive = jest.fn<boolean | null, []>(() => true);
  const hook = await renderHook(
    (props: Props) => useAdapterReconnect({ ...props, retry, end, isAlive }),
    { initialProps: initial },
  );
  const move = async (props: Props) => {
    await act(async () => {
      await hook.rerender(props);
    });
  };
  const sendAppState = async (state: AppStateStatus) => {
    await act(async () => {
      appState?.(state);
    });
  };
  return { ...hook, retry, end, isAlive, move, sendAppState };
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('retries a drop three times with growing pauses, then gives up', async () => {
  const { result, retry, move } = await setup();

  await move(DROPPED);
  expect(result.current).toMatchObject({ active: true, attempt: 1, max: 3 });
  await advance(999);
  expect(retry).not.toHaveBeenCalled();
  await advance(1);
  expect(retry).toHaveBeenCalledTimes(1);

  await move({ phase: 'connecting', error: null });
  expect(result.current.active).toBe(true);
  await move(DROPPED);
  expect(result.current.attempt).toBe(2);
  await advance(1999);
  expect(retry).toHaveBeenCalledTimes(1);
  await advance(1);
  expect(retry).toHaveBeenCalledTimes(2);

  await move({ phase: 'opening', error: null });
  await move(DROPPED);
  expect(result.current.attempt).toBe(3);
  await advance(4000);
  expect(retry).toHaveBeenCalledTimes(3);

  await move({ phase: 'opening', error: null });
  await move(DROPPED);
  expect(result.current.active).toBe(false);
  await advance(20000);
  expect(retry).toHaveBeenCalledTimes(3);
});

it('counts as reconnecting in the same render that reports the drop', async () => {
  const renders: { phase: AdapterUiPhase; active: boolean }[] = [];
  const hook = await renderHook(
    (props: Props) => {
      const reconnect = useAdapterReconnect({
        ...props,
        retry: jest.fn(),
        end: jest.fn(),
        isAlive: () => true,
      });
      renders.push({ phase: props.phase, active: reconnect.active });
      return reconnect;
    },
    { initialProps: STREAMING },
  );

  await act(async () => {
    await hook.rerender(DROPPED);
  });
  // Without this the error page would flash for one frame.
  expect(renders.filter(render => render.phase === 'ended')).not.toContainEqual(
    { phase: 'ended', active: false },
  );
});

it('starts over after a retry reaches the picture again', async () => {
  const { result, retry, move } = await setup();
  await move(DROPPED);
  await advance(1000);
  await move(STREAMING);
  expect(result.current.active).toBe(false);

  await move(DROPPED);
  expect(result.current.attempt).toBe(1);
  await advance(1000);
  expect(retry).toHaveBeenCalledTimes(2);
});

it.each([
  ['authFailed', ADAPTER_RS_E.authFailed],
  ['certRejected', ADAPTER_RS_E.certRejected],
  ['refused', ADAPTER_RS_E.serviceOff],
  ['serverClosed', ADAPTER_RS_E.broken],
] as const)('leaves %s to the user', async (code, rsCode) => {
  const { result, retry, move } = await setup();
  await move({ phase: 'ended', error: { code, rsCode, message: code } });

  expect(result.current.active).toBe(false);
  await advance(10000);
  expect(retry).not.toHaveBeenCalled();
});

it('does not retry a session that never showed a picture', async () => {
  const { result, retry, move } = await setup({
    phase: 'connecting',
    error: null,
  });
  await move(DROPPED);

  expect(result.current.active).toBe(false);
  await advance(10000);
  expect(retry).not.toHaveBeenCalled();
});

it('a clean end is not retried', async () => {
  const { result, retry, move } = await setup();
  await move({ phase: 'ended', error: null });

  expect(result.current.active).toBe(false);
  await advance(10000);
  expect(retry).not.toHaveBeenCalled();
});

it('cancel between attempts stops without closing anything', async () => {
  const { result, retry, end, move } = await setup();
  await move(DROPPED);

  await act(async () => result.current.cancel());
  expect(result.current.active).toBe(false);
  expect(end).not.toHaveBeenCalled();
  await advance(10000);
  expect(retry).not.toHaveBeenCalled();
  // The page shows the drop; the hook stays quiet for the same error.
  await move(DROPPED);
  expect(result.current.active).toBe(false);
});

it('cancel during an attempt ends it with the original error', async () => {
  const { result, end, move } = await setup();
  await move(DROPPED);
  await advance(1000);
  await move({ phase: 'connecting', error: null });

  await act(async () => result.current.cancel());
  expect(end).toHaveBeenCalledWith(DROP);
  expect(result.current.active).toBe(false);
});

it('waits for the foreground before retrying', async () => {
  const { retry, move, sendAppState } = await setup();
  await move(DROPPED);
  await sendAppState('background');

  await advance(10000);
  expect(retry).not.toHaveBeenCalled();
  await sendAppState('active');
  await advance(1000);
  expect(retry).toHaveBeenCalledTimes(1);
});

it('does not start a retry while the app is in the background', async () => {
  const original = Object.getOwnPropertyDescriptor(AppState, 'currentState');
  let current: AppStateStatus = 'background';
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    get: () => current,
  });
  try {
    const { result, retry, move, sendAppState } = await setup();
    await move(DROPPED);

    expect(result.current.active).toBe(true);
    await advance(10000);
    expect(retry).not.toHaveBeenCalled();
    current = 'active';
    await sendAppState('active');
    await advance(1000);
    expect(retry).toHaveBeenCalledTimes(1);
  } finally {
    if (original !== undefined) {
      Object.defineProperty(AppState, 'currentState', original);
    }
  }
});

it('ends a session that died while the app slept', async () => {
  const { end, isAlive, sendAppState } = await setup();

  await sendAppState('active');
  expect(end).not.toHaveBeenCalled();

  isAlive.mockReturnValue(null);
  await sendAppState('active');
  expect(end).not.toHaveBeenCalled();

  isAlive.mockReturnValue(false);
  await sendAppState('active');
  expect(end).toHaveBeenCalledWith(LOST_IN_BACKGROUND);
});
