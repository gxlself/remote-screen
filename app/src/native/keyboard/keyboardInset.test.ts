import { act, renderHook } from '@testing-library/react-native';
import {
  KEYBAR_HEIGHT,
  KEYBAR_HINT_HEIGHT,
  resetKeyboardInset,
  setKeyboardInset,
  useKeyboardInset,
} from './keyboardInset';

beforeEach(() => {
  resetKeyboardInset();
});

it('exposes the keyboard overlap plus the auxiliary bar', async () => {
  const hook = await renderHook(() => useKeyboardInset());
  expect(hook.result.current).toBe(0);
  await act(() => setKeyboardInset(300 + KEYBAR_HEIGHT + KEYBAR_HINT_HEIGHT));
  expect(hook.result.current).toBeGreaterThan(300);
  await act(() => resetKeyboardInset());
  expect(hook.result.current).toBe(0);
});

it('ignores invalid inset values', async () => {
  const hook = await renderHook(() => useKeyboardInset());
  await act(() => setKeyboardInset(Number.NaN));
  expect(hook.result.current).toBe(0);
  await act(() => setKeyboardInset(-10));
  expect(hook.result.current).toBe(0);
});
