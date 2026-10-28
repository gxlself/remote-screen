import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { RemoteVideoStats } from '../../native/session/RemoteSessionView';
import {
  clearVideo,
  setVideoStats,
  setViewport,
  useVideoStats,
  useViewport,
  videoStore,
} from './videoStore';

const stats: RemoteVideoStats = {
  fps: 60,
  decodeP50Ms: 1,
  decodeP95Ms: 2,
  latencyP50Ms: -1,
  queue: 0,
  dropped: 0,
  keyframeRequests: 0,
  width: 1920,
  height: 1080,
  codec: 'hevc',
};
const viewport = { zoom: 2, x: 0.25, y: 0.5 };

beforeEach(() => {
  videoStore.setState({ stats: {}, viewport: {} });
});

afterEach(async () => {
  await cleanup();
});

it('sets, replaces and clears native video data', () => {
  setVideoStats('s1', stats);
  setViewport('s1', viewport);
  expect(videoStore.getState()).toEqual({
    stats: { s1: stats },
    viewport: { s1: viewport },
  });
  const nextStats = { ...stats, fps: 30 };
  const nextViewport = { zoom: 1, x: 0, y: 0 };
  setVideoStats('s1', nextStats);
  setViewport('s1', nextViewport);
  expect(videoStore.getState()).toEqual({
    stats: { s1: nextStats },
    viewport: { s1: nextViewport },
  });
  clearVideo('s1');
  expect(videoStore.getState()).toEqual({ stats: {}, viewport: {} });
  const empty = videoStore.getState();
  clearVideo('s1');
  expect(videoStore.getState()).toBe(empty);
});

it('keeps updates and clearing isolated between sessions', () => {
  const otherStats = { ...stats, fps: 30 };
  const otherViewport = { zoom: 3, x: 0.5, y: 0.75 };
  setVideoStats('s1', stats);
  setViewport('s1', viewport);
  setVideoStats('s2', otherStats);
  setViewport('s2', otherViewport);
  setVideoStats('s1', { ...stats, dropped: 1 });
  setViewport('s1', { ...viewport, zoom: 4 });
  expect(videoStore.getState().stats.s2).toBe(otherStats);
  expect(videoStore.getState().viewport.s2).toBe(otherViewport);
  clearVideo('s1');
  expect(videoStore.getState()).toEqual({
    stats: { s2: otherStats },
    viewport: { s2: otherViewport },
  });
});

it('subscribes to native updates and returns null for absent or null ids', async () => {
  const { result, rerender } = await renderHook(
    ({ id }: { id: string | null }) => ({
      stats: useVideoStats(id),
      viewport: useViewport(id),
    }),
    { initialProps: { id: 's1' as string | null } },
  );
  expect(result.current).toEqual({ stats: null, viewport: null });
  await act(() => {
    setVideoStats('s1', stats);
    setViewport('s1', viewport);
  });
  expect(result.current).toEqual({ stats, viewport });
  await rerender({ id: null });
  expect(result.current).toEqual({ stats: null, viewport: null });
  await act(() => setVideoStats('s1', { ...stats, fps: 30 }));
  expect(result.current).toEqual({ stats: null, viewport: null });
  await rerender({ id: 's1' });
  expect(result.current.stats?.fps).toBe(30);
  await act(() => clearVideo('s1'));
  expect(result.current).toEqual({ stats: null, viewport: null });
});
