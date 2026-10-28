import type { SessionController } from '../../screens/Session/useSessionController';
import { useEffect } from 'react';
import { useCore } from '../../native/rscore/CoreContext';
import { useCoreEvent } from '../events/EventsProvider';
import { recording } from '../../i18n/zh/recording';
import {
  applyRecordingEvent,
  setRecordingUi,
  stopReasonText,
} from './sessionRecording';

export function useSessionRecording(ctx: SessionController): void {
  const core = useCore();
  const recordingApi = core.recording;
  const sessionId = ctx.sessionId;

  useEffect(() => {
    if (recordingApi === undefined) {
      return;
    }

    // 先清掉旧会话，再同步原生状态，避免状态切换期间短暂显示旧录制。
    setRecordingUi({
      sessionId,
      state: 'idle',
      recordingId: undefined,
      durationMs: 0,
      bytes: 0,
    });
    if (sessionId === null) {
      return;
    }

    try {
      const status = recordingApi.status(sessionId);
      if (status.state === 'recording' || status.state === 'stopping') {
        setRecordingUi({
          sessionId,
          state: status.state,
          recordingId: status.recordingId,
          durationMs: status.durationMs ?? 0,
          bytes: status.bytes ?? 0,
        });
      }
    } catch {
      // 旧原生包或会话刚结束时查询失败，按 idle 处理。
    }
  }, [recordingApi, sessionId]);

  useCoreEvent('recording.state', event => {
    if (
      recordingApi === undefined ||
      sessionId === null ||
      event.sessionId !== sessionId
    ) {
      return;
    }
    const result = applyRecordingEvent(event);
    if (result === 'failed') {
      ctx.showToast(recording.stopError, { tone: 'bad' });
      return;
    }
    if (result !== 'stopped') {
      return;
    }
    const reason = stopReasonText(event.reason);
    ctx.showToast(reason ?? recording.stoppedSaved, { tone: 'good' });
  });
}
