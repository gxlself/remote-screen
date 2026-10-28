import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useNow, useRemaining } from './time';

describe('time hooks', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 9, 20, 15, 0));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('updates useNow at the requested interval', () => {
    const start = Date.now();
    const { result } = renderHook(() => useNow(1000));
    expect(result.current).toBe(start);

    act(() => vi.advanceTimersByTime(1000));
    expect(result.current).toBe(start + 1000);
  });

  it('clears the useNow timer on unmount', () => {
    const { unmount } = renderHook(() => useNow(1000));
    expect(vi.getTimerCount()).toBe(1);

    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('tracks remaining time, including negative values', () => {
    const deadline = Date.now() + 5000;
    const { result } = renderHook(() => useRemaining(deadline));
    expect(result.current).toBe(5000);

    act(() => vi.advanceTimersByTime(250));
    expect(result.current).toBe(4750);

    act(() => vi.advanceTimersByTime(6000));
    expect(result.current).toBeLessThan(0);
  });

  it('returns null for a null deadline', () => {
    const { result } = renderHook(() => useRemaining(null));
    expect(result.current).toBeNull();
  });
});
