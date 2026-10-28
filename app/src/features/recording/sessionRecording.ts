import { useSyncExternalStore } from 'react';
import type { Computer, CoreEvent, SessionId } from '../../native/rscore/types';
import { hasHostFeature } from '../../native/rscore/types';
import { recording } from '../../i18n/zh/recording';
import { RECORDING_NOTICE_BIT } from './noticeBit';

export { RECORDING_NOTICE_BIT } from './noticeBit';

export type NoticePolicy = 'A' | 'B';
export const RECORDING_NOTICE_POLICY: NoticePolicy = 'A';

export type RecordingUiState =
  | 'idle'
  | 'confirm'
  | 'starting'
  | 'recording'
  | 'stopping';

export type RecordingUi = {
  sessionId: SessionId | null;
  state: RecordingUiState;
  recordingId?: string;
  durationMs: number;
  bytes: number;
};

const EMPTY_UI: RecordingUi = {
  sessionId: null,
  state: 'idle',
  durationMs: 0,
  bytes: 0,
};

let ui: RecordingUi = EMPTY_UI;
const listeners = new Set<() => void>();

function emit(next: RecordingUi): void {
  if (
    ui.sessionId === next.sessionId &&
    ui.state === next.state &&
    ui.recordingId === next.recordingId &&
    ui.durationMs === next.durationMs &&
    ui.bytes === next.bytes
  ) {
    return;
  }
  ui = next;
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot(): RecordingUi {
  return ui;
}

export function getRecordingUi(): RecordingUi {
  return ui;
}

export function useRecordingUi(): RecordingUi {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}

/** 更新状态；换会话时强制从 idle 开始，避免旧会话的录制状态串到新会话。 */
export function setRecordingUi(next: Partial<RecordingUi>): void {
  const sessionChanged =
    next.sessionId !== undefined && next.sessionId !== ui.sessionId;
  if (sessionChanged) {
    emit({
      sessionId: next.sessionId ?? null,
      state: 'idle',
      durationMs: 0,
      bytes: 0,
    });
    return;
  }

  const merged: RecordingUi = { ...ui, ...next };
  if (merged.state === 'idle' && next.recordingId === undefined) {
    delete merged.recordingId;
  }
  emit(merged);
}

/** 事件处理只负责转换状态；会话过滤由 useSessionRecording 完成。 */
export function applyRecordingEvent(
  e: Extract<CoreEvent, { type: 'recording.state' }>,
): 'stopped' | 'failed' | null {
  const base: RecordingUi =
    ui.sessionId === e.sessionId
      ? ui
      : {
          sessionId: e.sessionId,
          state: 'idle',
          durationMs: 0,
          bytes: 0,
        };
  if (e.state === 'recording') {
    emit({
      ...base,
      sessionId: e.sessionId,
      state: 'recording',
      recordingId: e.recordingId,
      durationMs: e.durationMs,
      bytes: e.bytes,
    });
    return null;
  }

  emit({
    sessionId: e.sessionId,
    state: 'idle',
    durationMs: e.durationMs,
    bytes: e.bytes,
  });
  return e.state === 'stopped' ? 'stopped' : 'failed';
}

export function resetRecordingUiForTests(): void {
  emit({ ...EMPTY_UI });
}

export function hostShowsNotice(
  host: Computer | undefined,
  policy: NoticePolicy = RECORDING_NOTICE_POLICY,
  bit: number | null = RECORDING_NOTICE_BIT,
): boolean {
  if (policy !== 'A' || host === undefined || bit === null) {
    return false;
  }
  return hasHostFeature(host.features, bit);
}

export type ConfirmKind = 'intro' | 'hostWontKnow' | null;

/** 是否需要确认框，需要的话用哪一套文案。 */
export function confirmKind(input: {
  introShown: boolean;
  host: Computer | undefined;
  policy?: NoticePolicy;
  bit?: number | null;
}): ConfirmKind {
  const policy = input.policy ?? RECORDING_NOTICE_POLICY;
  if (policy === 'A' && !hostShowsNotice(input.host, policy, input.bit)) {
    return 'hostWontKnow';
  }
  return input.introShown ? null : 'intro';
}

export function formatElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const two = (value: number) => String(value).padStart(2, '0');
  return hours > 0
    ? `${hours}:${two(minutes)}:${two(seconds)}`
    : `${minutes}:${two(seconds)}`;
}

function messageCode(error: unknown): string | null {
  if (error === null || typeof error !== 'object') {
    return typeof error === 'string' ? error : null;
  }
  const message = (error as { message?: unknown }).message;
  return typeof message === 'string' ? message : null;
}

export function startErrorText(error: unknown): string {
  const code = messageCode(error) ?? '';
  if (code.startsWith('recording.unsupported')) {
    return recording.errUnsupported;
  }
  if (code.startsWith('recording.keyUnavailable')) {
    return recording.errKeyUnavailable;
  }
  if (code.startsWith('recording.notStreaming')) {
    return recording.errNotStreaming;
  }
  if (code.startsWith('recording.alreadyRecording')) {
    return recording.errAlready;
  }
  if (code.startsWith('recording.storageFull')) {
    return recording.errStorageFull;
  }
  if (code.startsWith('recording.diskLow')) {
    return recording.errDiskLow;
  }
  if (
    typeof error === 'object' &&
    error !== null &&
    (error as { kind?: unknown }).kind === 'unsupported'
  ) {
    return recording.errUnsupported;
  }
  return recording.errGeneric;
}

export function stopReasonText(reason: string | undefined): string | null {
  switch (reason) {
    case undefined:
    case 'user':
      return null;
    case 'maxDuration':
      return recording.stopMaxDuration;
    case 'storageFull':
      return recording.stopStorageFull;
    case 'diskLow':
      return recording.stopDiskLow;
    case 'sessionEnded':
      return recording.stopSessionEnded;
    default:
      return recording.stopError;
  }
}
