import { useCallback, useEffect, useRef, useState } from 'react';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { recordings } from '../../i18n/zh/recordings';
import { startAudio, stopAudio } from '../../native/audio/audio';
import { useCore } from '../../native/rscore/CoreContext';
import type {
  CoreEventOf,
  PlaybackCommand,
  PlaybackId,
  PlaybackInfo,
} from '../../native/rscore/types';

export type PlaybackState = CoreEventOf<'playback.state'>['state'];

export type Playback = {
  info: PlaybackInfo | null;
  state: PlaybackState;
  positionMs: number;
  durationMs: number;
  error?: string;
  play(): void;
  pause(): void;
  seek(ms: number): void;
};

type Progress = Pick<Playback, 'state' | 'positionMs' | 'durationMs' | 'error'>;

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 打开一份录制并跟随 `playback.state`。卸载时关闭回放和声音。
 * `onControlError` 收到要提示的文案（播放命令被 Rust 拒绝时）。
 */
export function usePlayback(
  recordingId: string,
  onControlError: (message: string) => void,
): Playback {
  const core = useCore();
  const [info, setInfo] = useState<PlaybackInfo | null>(null);
  const [progress, setProgress] = useState<Progress>({
    state: 'loading',
    positionMs: 0,
    durationMs: 0,
  });
  const playbackId = useRef<PlaybackId | null>(null);
  // 事件和 open 的返回值走不同通道；open 返回前先收下同一录制的事件。
  const early = useRef(new Map<PlaybackId, CoreEventOf<'playback.state'>>());
  const progressRef = useRef(progress);
  progressRef.current = progress;
  const errorRef = useRef(onControlError);
  errorRef.current = onControlError;

  const apply = useCallback((event: CoreEventOf<'playback.state'>) => {
    setProgress({
      state: event.state,
      positionMs: event.positionMs,
      durationMs: event.durationMs,
      error: event.state === 'error' ? event.error : undefined,
    });
  }, []);

  useCoreEvent('playback.state', event => {
    if (playbackId.current === null) {
      if (event.recordingId === recordingId) {
        early.current.set(event.playbackId, event);
      }
      return;
    }
    if (event.playbackId === playbackId.current) {
      apply(event);
    }
  });

  useEffect(() => {
    const api = core.recording;
    const pending = early.current;
    if (api === undefined) {
      setProgress(current => ({
        ...current,
        state: 'error',
        error: 'recording.unsupported',
      }));
      return;
    }
    let active = true;
    let opened: PlaybackId | null = null;
    let audio = false;
    api.open(recordingId).then(
      next => {
        if (!active) {
          try {
            api.close(next.playbackId);
          } catch {
            // The page is gone; a failed close has nowhere to be shown.
          }
          return;
        }
        opened = next.playbackId;
        playbackId.current = next.playbackId;
        setInfo(next);
        setProgress(current => ({ ...current, durationMs: next.durationMs }));
        const seen = pending.get(next.playbackId);
        pending.clear();
        if (seen !== undefined) {
          apply(seen);
        }
        if (next.hasAudio) {
          audio = true;
          startAudio(next.playbackId);
        }
      },
      error => {
        if (active) {
          setProgress(current => ({
            ...current,
            state: 'error',
            error: messageOf(error),
          }));
        }
      },
    );
    return () => {
      active = false;
      playbackId.current = null;
      pending.clear();
      if (opened !== null) {
        try {
          api.close(opened);
        } catch {
          // Closing is best effort when leaving the page.
        }
      }
      if (audio) {
        stopAudio();
      }
    };
  }, [apply, core, recordingId]);

  const send = useCallback(
    (command: PlaybackCommand, optimistic: Partial<Progress>) => {
      const api = core.recording;
      const id = playbackId.current;
      if (api === undefined || id === null) {
        return;
      }
      const previous = progressRef.current;
      setProgress(current => ({ ...current, ...optimistic }));
      try {
        api.control(id, command);
      } catch {
        setProgress(previous);
        errorRef.current(recordings.player.loadFailed);
      }
    },
    [core],
  );

  const play = useCallback(() => {
    const ended = progressRef.current.state === 'ended';
    send(
      { op: 'play' },
      ended ? { state: 'playing', positionMs: 0 } : { state: 'playing' },
    );
  }, [send]);

  const pause = useCallback(() => {
    const { state } = progressRef.current;
    send({ op: 'pause' }, state === 'playing' ? { state: 'paused' } : {});
  }, [send]);

  const seek = useCallback(
    (ms: number) => {
      const max = progressRef.current.durationMs;
      const positionMs = Math.round(Math.min(Math.max(ms, 0), max));
      send({ op: 'seek', positionMs }, { positionMs });
    },
    [send],
  );

  return { info, ...progress, play, pause, seek };
}
