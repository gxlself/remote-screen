import { useStore } from 'zustand';
import { createStore } from 'zustand/vanilla';
import type { SessionId } from '../../native/rscore/types';
import type { RemoteVideoStats } from '../../native/session/RemoteSessionView';

export type VideoViewport = { zoom: number; x: number; y: number };

type VideoState = {
  stats: Record<SessionId, RemoteVideoStats>;
  viewport: Record<SessionId, VideoViewport>;
};

export const videoStore = createStore<VideoState>(() => ({
  stats: {},
  viewport: {},
}));

export function setVideoStats(id: SessionId, stats: RemoteVideoStats): void {
  videoStore.setState(state => ({ stats: { ...state.stats, [id]: stats } }));
}

export function setViewport(id: SessionId, viewport: VideoViewport): void {
  videoStore.setState(state => ({
    viewport: { ...state.viewport, [id]: viewport },
  }));
}

export function clearVideo(id: SessionId): void {
  videoStore.setState(state => {
    if (state.stats[id] === undefined && state.viewport[id] === undefined) {
      return state;
    }
    const stats = { ...state.stats };
    const viewport = { ...state.viewport };
    delete stats[id];
    delete viewport[id];
    return { stats, viewport };
  });
}

export function useVideoStats(id: SessionId | null): RemoteVideoStats | null {
  return useStore(videoStore, state =>
    id === null ? null : state.stats[id] ?? null,
  );
}

export function useViewport(id: SessionId | null): VideoViewport | null {
  return useStore(videoStore, state =>
    id === null ? null : state.viewport[id] ?? null,
  );
}
