import { act, renderHook } from '@testing-library/react-native';
import type { HudModel } from './hud';
import { useHudExpansion } from './useHudExpansion';

function model(overrides: Partial<HudModel> = {}): HudModel {
  return {
    path: 'tailnetDirect',
    pathText: '直连',
    latencyMs: 6,
    fps: 60,
    tcp: false,
    alertText: null,
    a11y: '连接质量：直连 6 毫秒，60 帧，查看详情',
    ...overrides,
  };
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  jest.useRealTimers();
});

it('starts expanded and collapses after exactly three seconds', async () => {
  const { result } = await renderHook(() => useHudExpansion(model()));

  expect(result.current).toBe('expanded');
  await act(() => jest.advanceTimersByTime(2999));
  expect(result.current).toBe('expanded');
  await act(() => jest.advanceTimersByTime(1));
  expect(result.current).toBe('collapsed');
});

it('starts expanded even when the first model already has an alert', async () => {
  const { result } = await renderHook(() =>
    useHudExpansion(model({ alertText: '画质已自动降低' })),
  );

  expect(result.current).toBe('expanded');
  await act(() => jest.advanceTimersByTime(3000));
  expect(result.current).toBe('collapsed');
});

it('does not restart its timer for an unchanged model', async () => {
  const { result, rerender } = await renderHook(
    (current: HudModel) => useHudExpansion(current),
    { initialProps: model() },
  );

  await act(() => jest.advanceTimersByTime(2000));
  await rerender(model());
  await act(() => jest.advanceTimersByTime(1000));
  expect(result.current).toBe('collapsed');
});

it('expands for three more seconds when the path changes', async () => {
  const { result, rerender } = await renderHook(
    (current: HudModel) => useHudExpansion(current),
    { initialProps: model() },
  );

  await act(() => jest.advanceTimersByTime(3000));
  expect(result.current).toBe('collapsed');

  await rerender(model({ path: 'tailnetDerp', pathText: '中继 · 上海' }));
  expect(result.current).toBe('expanded');
  await act(() => jest.advanceTimersByTime(2999));
  expect(result.current).toBe('expanded');
  await act(() => jest.advanceTimersByTime(1));
  expect(result.current).toBe('collapsed');
});

it('expands when the transport switches to TCP', async () => {
  const { result, rerender } = await renderHook(
    (current: HudModel) => useHudExpansion(current),
    { initialProps: model() },
  );

  await act(() => jest.advanceTimersByTime(3000));
  await rerender(
    model({ path: 'intranet', pathText: '内网 · TCP', tcp: true }),
  );
  expect(result.current).toBe('expanded');
});

it('shows a new rate-limit alert for five seconds', async () => {
  const { result, rerender } = await renderHook(
    (current: HudModel) => useHudExpansion(current),
    { initialProps: model() },
  );

  await act(() => jest.advanceTimersByTime(3000));
  expect(result.current).toBe('collapsed');

  await rerender(model({ alertText: '画质已自动降低' }));
  expect(result.current).toBe('alert');
  await act(() => jest.advanceTimersByTime(4999));
  expect(result.current).toBe('alert');
  await act(() => jest.advanceTimersByTime(1));
  expect(result.current).toBe('collapsed');
});
