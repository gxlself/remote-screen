import { act, renderHook } from '@testing-library/react-native';
import {
  resetTerminalKeyboard,
  setTerminalKeyboard,
  useTerminalKeyboard,
} from './terminalKeyboard';

beforeEach(() => {
  resetTerminalKeyboard();
});

it('exposes keyboard visibility and overlap', async () => {
  const hook = await renderHook(() => useTerminalKeyboard());
  expect(hook.result.current).toEqual({ visible: false, overlap: 0 });

  await act(() => setTerminalKeyboard(true, 336));
  expect(hook.result.current).toEqual({ visible: true, overlap: 336 });
});

it('normalizes invalid overlap values and resets the store', async () => {
  const hook = await renderHook(() => useTerminalKeyboard());
  await act(() => setTerminalKeyboard(true, Number.NaN));
  expect(hook.result.current).toEqual({ visible: true, overlap: 0 });

  await act(() => setTerminalKeyboard(true, -12));
  expect(hook.result.current).toEqual({ visible: true, overlap: 0 });

  await act(() => resetTerminalKeyboard());
  expect(hook.result.current).toEqual({ visible: false, overlap: 0 });
});
