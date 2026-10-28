import { useState } from 'react';
import { Pressable, StyleSheet } from 'react-native';
import { Dialog } from '../../components/Dialog';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { useSetting } from '../settings/store';
import { useCore } from '../../native/rscore/CoreContext';
import { recording } from '../../i18n/zh/recording';
import type { SlotProps } from '../../screens/Session/slots';
import { hit } from '../../theme/tokens';
import {
  confirmKind,
  formatElapsed,
  RECORDING_NOTICE_POLICY,
  setRecordingUi,
  useRecordingUi,
} from './sessionRecording';
import { startRecording, stopRecording } from './RecordingMoreRow';

const CHIP_HEIGHT = 32;

export function RecordingChip({ ctx }: SlotProps) {
  const core = useCore();
  const [introShown, setIntroShown] = useSetting('recording.introShown');
  const [confirmStop, setConfirmStop] = useState(false);
  const state = useRecordingUi();
  const recordingApi = core.recording;
  const sessionId = ctx.sessionId;

  if (
    recordingApi === undefined ||
    sessionId === null ||
    state.sessionId !== sessionId
  ) {
    return null;
  }

  if (
    state.state === 'starting' ||
    state.state === 'recording' ||
    state.state === 'stopping'
  ) {
    const elapsed = formatElapsed(state.durationMs);
    const active = state.state === 'recording';
    const label =
      state.state === 'starting'
        ? recording.chipStarting
        : state.state === 'stopping'
        ? recording.chipStopping
        : `${recording.chip} ${elapsed}`;
    const slop = Math.max(0, (hit.min - CHIP_HEIGHT) / 2);
    return (
      <>
        <Pressable
          accessibilityHint={active ? recording.chipHint : undefined}
          accessibilityLabel={active ? recording.chipA11y(elapsed) : label}
          accessibilityRole="button"
          accessibilityState={{ disabled: !active, busy: !active }}
          disabled={!active}
          hitSlop={{ top: slop, bottom: slop }}
          onPress={() => setConfirmStop(true)}
          style={({ pressed }) => [
            styles.chip,
            pressed ? styles.pressed : null,
          ]}
          testID="recording-chip"
        >
          <Icon color="bad" name="record" size={12} />
          <AppText color="bad" style={styles.label} variant="caption">
            {label}
          </AppText>
        </Pressable>
        {confirmStop && active ? (
          <Dialog
            actions={[
              {
                label: recording.cancel,
                onPress: () => setConfirmStop(false),
                variant: 'secondary',
              },
              {
                label: recording.stopConfirm,
                onPress: () => {
                  setConfirmStop(false);
                  stopRecording(core, ctx);
                },
                variant: 'pillDanger',
              },
            ]}
            message={recording.stopConfirmBody(elapsed)}
            onDismiss={() => setConfirmStop(false)}
            testID="recording-stop-confirm"
            title={recording.stopConfirmTitle}
            visible
          />
        ) : null}
      </>
    );
  }

  if (state.state !== 'confirm') {
    return null;
  }

  const kind = confirmKind({
    introShown,
    host: ctx.host,
    policy: RECORDING_NOTICE_POLICY,
  });
  const resolvedKind = kind ?? 'intro';
  const title =
    resolvedKind === 'intro'
      ? recording.introTitle
      : recording.hostWontKnowTitle;
  const message =
    resolvedKind === 'intro'
      ? RECORDING_NOTICE_POLICY === 'A'
        ? recording.introBodyA
        : recording.introBodyB
      : recording.hostWontKnowBody;
  const cancel = () => {
    setRecordingUi({
      state: 'idle',
      recordingId: undefined,
      durationMs: 0,
      bytes: 0,
    });
  };

  return (
    <Dialog
      actions={[
        { label: recording.cancel, onPress: cancel, variant: 'secondary' },
        {
          label: recording.confirmStart,
          onPress: () => {
            setIntroShown(true);
            startRecording(core, ctx).catch(() => undefined);
          },
        },
      ]}
      message={message}
      onDismiss={cancel}
      testID="recording-confirm"
      title={title}
      visible
    />
  );
}

export default RecordingChip;

const styles = StyleSheet.create({
  // 和 HUD 胶囊并排放在左上角，位置由 SessionHud 决定。
  chip: {
    height: CHIP_HEIGHT,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    paddingHorizontal: 12,
    borderRadius: CHIP_HEIGHT / 2,
    backgroundColor: 'rgba(32, 12, 16, 0.92)',
  },
  pressed: { opacity: 0.7 },
  label: { fontSize: 12, lineHeight: 16 },
});
