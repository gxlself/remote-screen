import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { IconButton } from '../../components/IconButton';
import { ListRow } from '../../components/List';
import { StatusChip } from '../../components/StatusChip';
import { recordings } from '../../i18n/zh/recordings';
import type { RecordingInfo } from '../../native/rscore/types';
import {
  formatBytes,
  formatDuration,
  formatStartedAt,
  stateBadge,
} from '../../features/recording/library';
import { tokens } from '../../theme/tokens';

export type RecordingRowProps = {
  recording: RecordingInfo;
  nowMs: number;
  onPress: () => void;
  onDelete: () => void;
  deleting?: boolean;
};

export function RecordingRow({
  recording,
  nowMs,
  onPress,
  onDelete,
  deleting = false,
}: RecordingRowProps) {
  const hostName = recording.hostName.trim() || recordings.unknownHost;
  const subtitle = [
    formatStartedAt(recording.startedAt, nowMs),
    formatDuration(recording.durationMs),
    formatBytes(recording.bytes),
  ].join(' · ');
  const badge = stateBadge(recording.state);
  const isRecording = recording.state === 'recording';

  return (
    <ListRow
      accessibilityLabel={`${hostName}，${subtitle}`}
      disabled={isRecording || deleting}
      onPress={isRecording ? () => undefined : onPress}
      right={
        <View style={styles.actions}>
          {badge === null ? null : badge.tone === 'bad' ? (
            <View style={styles.badge}>
              <AppText color="bad" variant="tiny" weight={500}>
                {badge.label}
              </AppText>
            </View>
          ) : (
            <StatusChip label={badge.label} size="sm" tone="neutral" />
          )}
          {isRecording ? null : (
            <IconButton
              accessibilityLabel={recordings.a11y.delete}
              disabled={deleting}
              icon="trash"
              onPress={onDelete}
              size={36}
              testID={`recording-delete-${recording.recordingId}`}
              variant="plain"
            />
          )}
        </View>
      }
      testID={`recording-row-${recording.recordingId}`}
      title={hostName}
      subtitle={subtitle}
    />
  );
}

const styles = StyleSheet.create({
  actions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  badge: {
    borderRadius: 10,
    backgroundColor: tokens.badSoft,
    paddingHorizontal: 7,
    paddingVertical: 3,
  },
});

export default RecordingRow;
