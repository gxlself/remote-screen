import type { ReactElement } from 'react';
import { ListRow } from '../../components/List';
import { Icon } from '../../components/icons/Icon';
import { useSetting } from '../settings/store';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { SessionCtx } from '../../screens/Session/slots';
import { recording } from '../../i18n/zh/recording';
import {
  confirmKind,
  formatElapsed,
  getRecordingUi,
  RECORDING_NOTICE_POLICY,
  setRecordingUi,
  startErrorText,
  useRecordingUi,
} from './sessionRecording';

/** 开始录制的唯一入口；确认框和「更多」行共用它。 */
export async function startRecording(
  core: RsCoreApi,
  ctx: Pick<SessionCtx, 'sessionId' | 'showToast'>,
): Promise<void> {
  const sessionId = ctx.sessionId;
  const recordingApi = core.recording;
  if (recordingApi === undefined || sessionId === null) {
    return;
  }

  if (getRecordingUi().sessionId !== sessionId) {
    setRecordingUi({ sessionId });
  }
  setRecordingUi({
    state: 'starting',
    recordingId: undefined,
    durationMs: 0,
    bytes: 0,
  });

  try {
    const status = await recordingApi.start(sessionId);
    // 会话已经切换时，旧请求的结果不能覆盖新会话的状态。
    if (getRecordingUi().sessionId !== sessionId) {
      return;
    }
    setRecordingUi({
      state: 'recording',
      recordingId: status.recordingId,
      durationMs: status.durationMs ?? 0,
      bytes: status.bytes ?? 0,
    });
    ctx.showToast(recording.started);
  } catch (error) {
    if (getRecordingUi().sessionId !== sessionId) {
      return;
    }
    setRecordingUi({
      state: 'idle',
      recordingId: undefined,
      durationMs: 0,
      bytes: 0,
    });
    ctx.showToast(startErrorText(error), { tone: 'bad' });
  }
}

/** 停止录制的唯一入口；录制芯片的确认框和「更多」行共用它。 */
export function stopRecording(
  core: RsCoreApi,
  ctx: Pick<SessionCtx, 'sessionId' | 'showToast'>,
): void {
  const sessionId = ctx.sessionId;
  const recordingApi = core.recording;
  if (recordingApi === undefined || sessionId === null) {
    return;
  }
  try {
    recordingApi.stop(sessionId);
  } catch {
    ctx.showToast(recording.stopFailed, { tone: 'bad' });
    return;
  }
  setRecordingUi({ state: 'stopping' });
}

function recordingRow(
  core: RsCoreApi,
  ctx: SessionCtx,
  onClose: () => void,
  state: ReturnType<typeof useRecordingUi>,
): ReactElement {
  const stopping = state.state === 'stopping';
  return (
    <ListRow
      disabled={stopping}
      left={<Icon color="bad" name="record" size={20} />}
      onPress={() => {
        if (stopping) {
          return;
        }
        onClose();
        stopRecording(core, ctx);
      }}
      subtitle={
        stopping
          ? recording.moreStopping
          : recording.moreStopSubtitle(formatElapsed(state.durationMs))
      }
      testID="more-recording-stop"
      title={recording.moreStop}
    />
  );
}

export function useRecordingMoreRow(
  ctx: SessionCtx,
  onClose: () => void,
): ReactElement | null {
  const core = useCore();
  const [introShown] = useSetting('recording.introShown');
  const state = useRecordingUi();
  const recordingApi = core.recording;
  const sessionId = ctx.sessionId;

  if (recordingApi === undefined || sessionId === null) {
    return null;
  }

  if (
    state.sessionId === sessionId &&
    (state.state === 'recording' || state.state === 'stopping')
  ) {
    return recordingRow(core, ctx, onClose, state);
  }

  if (state.sessionId === sessionId && state.state === 'starting') {
    return (
      <ListRow
        disabled
        left={<Icon color="text2" name="record" size={20} />}
        subtitle={recording.moreStarting}
        testID="more-recording-starting"
        title={recording.moreStart}
      />
    );
  }

  if (state.state !== 'idle' || ctx.rec?.phase !== 'streaming') {
    return null;
  }

  return (
    <ListRow
      left={<Icon color="text2" name="record" size={20} />}
      onPress={() => {
        onClose();
        if (getRecordingUi().sessionId !== sessionId) {
          setRecordingUi({ sessionId });
        }
        const kind = confirmKind({
          introShown,
          host: ctx.host,
          policy: RECORDING_NOTICE_POLICY,
        });
        if (kind !== null) {
          setRecordingUi({ state: 'confirm' });
          return;
        }
        startRecording(core, ctx).catch(() => undefined);
      }}
      subtitle={recording.moreStartSubtitle}
      testID="more-recording-start"
      title={recording.moreStart}
    />
  );
}
