import { useState } from 'react';
import { ActivityIndicator, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import { EmptyState } from '../../components/EmptyState';
import { IconButton } from '../../components/IconButton';
import { Toast, useToast } from '../../components/Toast';
import { exportRecording } from '../../features/recording/exportShare';
import {
  formatStartedAt,
  useRecordings,
} from '../../features/recording/library';
import { common } from '../../i18n/zh/common';
import { recordings } from '../../i18n/zh/recordings';
import { useCore } from '../../native/rscore/CoreContext';
import { RemoteSessionView } from '../../native/session/RemoteSessionView';
import { tokens } from '../../theme/tokens';
import { useReduceMotion } from '../SessionEnded/useReduceMotion';
import { PlayerControls } from './PlayerControls';
import { usePlayback } from './usePlayback';

const { player } = recordings;
const exportCopy = recordings.export;

function errorCopy(error: string | undefined): {
  title: string;
  message?: string;
} {
  if (error?.startsWith('recording.damaged')) {
    return { title: player.loadFailed, message: player.damagedNote };
  }
  if (error?.startsWith('recording.unsupported')) {
    return { title: recordings.unsupported };
  }
  return { title: player.loadFailed };
}

function isBusy(error: unknown): boolean {
  return error instanceof Error && error.message.startsWith('recording.busy');
}

export default function RecordingPlayerScreen({
  navigation,
  route,
}: RootScreenProps<'RecordingPlayer'>) {
  const { recordingId } = route.params;
  const core = useCore();
  const insets = useSafeAreaInsets();
  const reduceMotion = useReduceMotion();
  const { toast, show, hide } = useToast();
  const playback = usePlayback(recordingId, show);
  const item = useRecordings().data?.recordings.find(
    recording => recording.recordingId === recordingId,
  );
  const [exporting, setExporting] = useState(false);

  const title =
    item === undefined
      ? recordings.title
      : item.hostName.trim() || recordings.unknownHost;
  const hasAudio = playback.info?.hasAudio ?? item?.hasAudio ?? false;
  const failure = errorCopy(playback.error);
  const goBack = () => navigation.goBack();
  const edges = {
    paddingLeft: Math.max(insets.left, 16),
    paddingRight: Math.max(insets.right, 16),
  };

  const startExport = () => {
    if (exporting) {
      return;
    }
    playback.pause();
    setExporting(true);
    exportRecording(core, recordingId)
      .then(
        result => {
          if (result === 'saved') {
            show(
              hasAudio
                ? `${exportCopy.saved}，${exportCopy.noAudioNote}`
                : exportCopy.saved,
              { tone: 'good' },
            );
          } else if (result === 'unsupported') {
            show(exportCopy.unsupported);
          }
        },
        error => {
          show(isBusy(error) ? exportCopy.busy : exportCopy.failed, {
            tone: 'bad',
          });
        },
      )
      .finally(() => setExporting(false));
  };

  return (
    <View style={styles.root} testID="player-root">
      <View style={styles.layer} testID="player-video">
        <RemoteSessionView
          accessibilityLabel={player.videoA11y}
          keyboardInset={0}
          mode="touch"
          reduceMotion={reduceMotion}
          sessionId={playback.info?.playbackId ?? null}
          toolbarCollapsed={false}
        />
      </View>

      {playback.state === 'loading' ? (
        <View
          pointerEvents="none"
          style={[styles.layer, styles.center]}
          testID="player-loading"
        >
          <ActivityIndicator color={tokens.text} size="small" />
          <AppText color="text2" variant="footnote">
            {player.loading}
          </AppText>
        </View>
      ) : null}

      {playback.state === 'error' ? (
        <View
          style={[styles.layer, styles.center, styles.failed]}
          testID="player-error"
        >
          <EmptyState
            action={{ label: player.back, onPress: goBack }}
            icon="alert"
            message={failure.message}
            title={failure.title}
          />
        </View>
      ) : (
        <>
          <View
            pointerEvents="box-none"
            style={[styles.top, edges, { paddingTop: insets.top + 8 }]}
          >
            <View style={styles.topBar}>
              <IconButton
                accessibilityLabel={common.a11y.back}
                icon="chevronLeft"
                onPress={goBack}
                testID="player-back"
              />
              <View style={styles.heading}>
                <AppText accessibilityRole="header" variant="headline">
                  {title}
                </AppText>
                {item === undefined ? null : (
                  <AppText color="text2" variant="caption">
                    {formatStartedAt(item.startedAt, Date.now())}
                  </AppText>
                )}
              </View>
            </View>
            {item?.state === 'damaged' ? (
              <View style={styles.banner}>
                <Banner
                  icon="alert"
                  testID="player-damaged"
                  title={player.damagedNote}
                  tone="warn"
                />
              </View>
            ) : null}
          </View>
          <View
            pointerEvents="box-none"
            style={[
              styles.bottom,
              edges,
              { paddingBottom: Math.max(insets.bottom, 12) },
            ]}
          >
            <PlayerControls
              durationMs={playback.durationMs}
              exporting={exporting}
              onExport={startExport}
              onPause={playback.pause}
              onPlay={playback.play}
              onSeek={playback.seek}
              positionMs={playback.positionMs}
              ready={playback.info !== null}
              state={playback.state}
            />
          </View>
        </>
      )}

      {toast !== null && (
        <Toast
          offset={insets.top + 72}
          onHide={hide}
          position="top"
          toast={toast}
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: '#000' },
  layer: StyleSheet.absoluteFill,
  center: {
    alignItems: 'center',
    justifyContent: 'center',
    gap: 12,
    paddingHorizontal: 24,
  },
  failed: { backgroundColor: '#000' },
  top: {
    position: 'absolute',
    top: 0,
    left: 0,
    right: 0,
    gap: 8,
  },
  topBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
  },
  heading: {
    flex: 1,
    gap: 2,
  },
  banner: {
    alignSelf: 'flex-start',
    maxWidth: 560,
  },
  bottom: {
    position: 'absolute',
    left: 0,
    right: 0,
    bottom: 0,
  },
});
