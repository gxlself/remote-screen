import { act, cleanup, renderHook } from '@testing-library/react-native';
import { AccessibilityInfo } from 'react-native';
import { useScreenReader } from './useScreenReader';

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('reads the initial screen reader state and follows changes', async () => {
  const listener: { current: ((value: boolean) => void) | null } = {
    current: null,
  };
  const remove = jest.fn();
  jest
    .spyOn(AccessibilityInfo, 'addEventListener')
    .mockImplementation((_event, handler) => {
      listener.current = handler;
      return { remove };
    });

  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(true);
  const { result } = await renderHook(() => useScreenReader());
  expect(result.current).toBe(true);

  await act(async () => {
    listener.current?.(false);
  });
  expect(result.current).toBe(false);
  expect(remove).not.toHaveBeenCalled();
});

it('removes its listener on unmount', async () => {
  const remove = jest.fn();
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(false);

  const listener: { current: ((value: boolean) => void) | null } = {
    current: null,
  };
  jest
    .spyOn(AccessibilityInfo, 'addEventListener')
    .mockImplementation((_event, handler) => {
      listener.current = handler;
      return { remove };
    });

  const { result, unmount } = await renderHook(() => useScreenReader());
  expect(result.current).toBe(false);
  await unmount();
  expect(remove).toHaveBeenCalledTimes(1);
  await act(async () => {
    listener.current?.(true);
  });
  expect(result.current).toBe(false);
});
