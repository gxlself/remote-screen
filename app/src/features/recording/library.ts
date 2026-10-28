import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { UseMutationResult, UseQueryResult } from '@tanstack/react-query';
import { useCoreEvent } from '../events/EventsProvider';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type {
  RecordingFileState,
  RecordingList,
  RecordingStorage,
} from '../../native/rscore/types';
import type {
  RecordingCapMb,
  RecordingRetentionDays,
} from '../settings/schema';
import { common } from '../../i18n/zh/common';
import { recordings } from '../../i18n/zh/recordings';

export const RECORDINGS_QUERY_KEY = ['recordings', 'list'] as const;

export function useRecordings(): UseQueryResult<RecordingList> {
  const core = useCore();
  return useQuery<RecordingList>({
    queryKey: RECORDINGS_QUERY_KEY,
    queryFn: () => core.recording!.list(),
    enabled: core.recording !== undefined,
  });
}

export function useDeleteRecording(): UseMutationResult<void, unknown, string> {
  const core = useCore();
  const queryClient = useQueryClient();
  return useMutation<void, unknown, string>({
    mutationFn: recordingId => {
      if (core.recording === undefined) {
        return Promise.reject(
          new RsCoreError('unsupported', 'recording.unsupported'),
        );
      }
      return core.recording.remove(recordingId);
    },
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: RECORDINGS_QUERY_KEY }),
  });
}

/** 录制结束或失败后，列表重新读取已完成的文件和存储概况。 */
export function useRecordingsAutoRefresh(): void {
  const queryClient = useQueryClient();
  useCoreEvent('recording.state', event => {
    if (event.state === 'stopped' || event.state === 'failed') {
      queryClient.invalidateQueries({ queryKey: RECORDINGS_QUERY_KEY });
    }
  });
}

export function formatBytes(n: number): string {
  if (n < 1024) {
    return `${Math.max(0, Math.round(n))} B`;
  }
  const units = ['KB', 'MB', 'GB', 'TB'] as const;
  let value = n;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length) {
    value /= 1024;
    unitIndex += 1;
  }
  const unit = units[unitIndex - 1] ?? units[units.length - 1];
  return `${value.toFixed(1)} ${unit}`;
}

export function formatDuration(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const seconds = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const minutes = totalMinutes % 60;
  const hours = Math.floor(totalMinutes / 60);
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, '0')}:${String(
      seconds,
    ).padStart(2, '0')}`;
  }
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

function dateKey(date: Date): number {
  return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
}

function timePart(date: Date): string {
  return `${String(date.getHours()).padStart(2, '0')}:${String(
    date.getMinutes(),
  ).padStart(2, '0')}`;
}

export function formatStartedAt(us: number, nowMs: number): string {
  const started = new Date(us / 1000);
  const now = new Date(nowMs);
  const dayDelta = Math.round((dateKey(now) - dateKey(started)) / 86_400_000);
  if (dayDelta === 0) {
    return `${recordings.today} ${timePart(started)}`;
  }
  if (dayDelta === 1) {
    return `${recordings.yesterday} ${timePart(started)}`;
  }
  const date = common.time.monthDay(started.getMonth() + 1, started.getDate());
  if (started.getFullYear() !== now.getFullYear()) {
    return recordings.yearDate(started.getFullYear(), date);
  }
  return `${date} ${timePart(started)}`;
}

export function stateBadge(
  state: RecordingFileState,
): { label: string; tone: 'neutral' | 'bad' } | null {
  switch (state) {
    case 'complete':
      return null;
    case 'recovered':
      return { label: recordings.badgeRecovered, tone: 'neutral' };
    case 'damaged':
      return { label: recordings.badgeDamaged, tone: 'bad' };
    case 'recording':
      return { label: recordings.badgeRecording, tone: 'neutral' };
  }
}

function compactBytes(n: number): string {
  return formatBytes(n).replace(/\.0 (?=[KMGT]B$)/, ' ');
}

export function storageSummary(s: RecordingStorage): string {
  const summary = `${recordings.storageUsed(
    formatBytes(s.usedBytes),
    compactBytes(s.capBytes),
  )}`;
  return s.freeBytes === undefined
    ? summary
    : `${summary} · ${recordings.storageFree(formatBytes(s.freeBytes))}`;
}

export const CAP_OPTIONS: readonly RecordingCapMb[] = [
  1024, 2048, 4096, 8192,
] as const;

export const RETENTION_OPTIONS: readonly RecordingRetentionDays[] = [
  7,
  30,
  90,
  null,
] as const;

export function capLabel(mb: RecordingCapMb): string {
  return `${mb / 1024} GB`;
}

export function retentionLabel(d: RecordingRetentionDays): string {
  return d === null ? recordings.retentionForever : recordings.retentionDays(d);
}

export function listErrorText(error: unknown): string {
  const kind =
    error !== null && typeof error === 'object' && 'kind' in error
      ? (error as { kind: unknown }).kind
      : undefined;
  if (kind === 'unsupported') {
    return recordings.unsupported;
  }
  if (error instanceof RsCoreError) {
    if (error.kind === 'unsupported') {
      return recordings.unsupported;
    }
    if (error.message.includes('recording.keyUnavailable')) {
      return recordings.keyUnavailable;
    }
  }
  const message =
    error !== null && typeof error === 'object' && 'message' in error
      ? String((error as { message: unknown }).message)
      : String(error ?? '');
  if (message.includes('recording.keyUnavailable')) {
    return recordings.keyUnavailable;
  }
  return recordings.loadFailed;
}
