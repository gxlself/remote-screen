import { act, cleanup, renderHook } from '@testing-library/react-native';
import {
  clearPointerLock,
  normalizePointerLock,
  pointerChipKind,
  pointerLockStore,
  setPointerLock,
  usePointerLock,
} from './pointerLockStore';
import type { PointerLockState } from './pointerLockStore';

const state: PointerLockState = {
  mouse: true,
  locked: false,
  reason: 'notFullScreen',
};

beforeEach(() => {
  pointerLockStore.setState({ states: {} });
});

afterEach(async () => {
  await cleanup();
});

it('normalizes unknown reasons and coerces flags to booleans', () => {
  expect(
    normalizePointerLock({
      mouse: true,
      locked: false,
      reason: 'fullscreenOnly',
    }),
  ).toEqual({ mouse: true, locked: false, reason: 'pending' });
  expect(
    normalizePointerLock({
      mouse: 1 as unknown as boolean,
      locked: 0 as unknown as boolean,
      reason: 'escaped',
    }),
  ).toEqual({ mouse: true, locked: false, reason: 'escaped' });
});

it('sets, replaces and clears state per session without duplicate writes', () => {
  const listener = jest.fn();
  const unsubscribe = pointerLockStore.subscribe(listener);

  setPointerLock('s1', state);
  expect(pointerLockStore.getState().states.s1).toBe(state);
  expect(listener).toHaveBeenCalledTimes(1);

  setPointerLock('s1', { ...state });
  expect(listener).toHaveBeenCalledTimes(1);

  setPointerLock('s2', { ...state, reason: 'escaped' });
  expect(listener).toHaveBeenCalledTimes(2);
  expect(pointerLockStore.getState().states.s2?.reason).toBe('escaped');

  clearPointerLock('s1');
  expect(listener).toHaveBeenCalledTimes(3);
  expect(pointerLockStore.getState().states).toEqual({
    s2: { ...state, reason: 'escaped' },
  });

  clearPointerLock('absent');
  expect(listener).toHaveBeenCalledTimes(3);
  unsubscribe();
});

it('subscribes to state by session id', async () => {
  const { result, rerender } = await renderHook(
    ({ id }: { id: string | null }) => usePointerLock(id),
    { initialProps: { id: 's1' as string | null } },
  );
  expect(result.current).toBeNull();

  await act(() => setPointerLock('s1', state));
  expect(result.current).toEqual(state);
  await act(() => setPointerLock('s1', { ...state, locked: true }));
  expect(result.current?.locked).toBe(true);

  await rerender({ id: 's2' });
  expect(result.current).toBeNull();
  await rerender({ id: null });
  expect(result.current).toBeNull();

  await act(() => clearPointerLock('s1'));
  await rerender({ id: 's1' });
  expect(result.current).toBeNull();
});

it('only shows a chip for an unlocked mouse on iPad', () => {
  expect(pointerChipKind(state, false)).toBeNull();
  expect(pointerChipKind(null, true)).toBeNull();
  expect(pointerChipKind({ ...state, mouse: false }, true)).toBeNull();
  expect(pointerChipKind({ ...state, locked: true }, true)).toBeNull();
  expect(pointerChipKind(state, true)).toBe('split');
  expect(pointerChipKind({ ...state, reason: 'escaped' }, true)).toBe(
    'escaped',
  );
  expect(pointerChipKind({ ...state, reason: 'pending' }, true)).toBeNull();
});
