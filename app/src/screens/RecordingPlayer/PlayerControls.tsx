import { ActivityIndicator, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { IconButton } from '../../components/IconButton';
import { formatDuration } from '../../features/recording/library';
import { recordings } from '../../i18n/zh/recordings';
import { radius, tokens } from '../../theme/tokens';
import type { PlaybackState } from './usePlayback';
import { SeekBar } from './SeekBar';

const { player } = recordings;

export type PlayerControlsProps = {
  state: PlaybackState;
  positionMs: number;
  durationMs: number;
  ready: boolean;
  exporting: boolean;
  onPlay: () => void;
  onPause: () => void;
  onSeek: (ms: number) => void;
  onExport: () => void;
};

export function PlayerControls({
  state,
  positionMs,
  durationMs,
  ready,
  exporting,
  onPlay,
  onPause,
  onSeek,
  onExport,
}: PlayerControlsProps) {
  const playing = state === 'playing';

  return (
    <View style={styles.bar} testID="player-controls">
      {playing ? (
        <IconButton
          accessibilityLabel={player.a11y.pause}
          disabled={!ready}
          icon="pause"
          onPress={onPause}
          testID="player-toggle"
          variant="plain"
        />
      ) : (
        <IconButton
          accessibilityLabel={player.a11y.play}
          disabled={!ready}
          icon="play"
          onPress={onPlay}
          testID="player-toggle"
          variant="plain"
        />
      )}
      <SeekBar
        disabled={!ready}
        durationMs={durationMs}
        onSeek={onSeek}
        positionMs={positionMs}
      />
      <AppText
        color="text2"
        mono
        style={styles.position}
        testID="player-position"
        variant="footnote"
      >
        {player.position(
          formatDuration(positionMs),
          formatDuration(durationMs),
        )}
      </AppText>
      {exporting ? (
        <View
          accessibilityLiveRegion="polite"
          accessible
          accessibilityLabel={recordings.export.working}
          style={styles.working}
          testID="player-exporting"
        >
          <ActivityIndicator color={tokens.text2} size="small" />
          <AppText color="text2" variant="footnote">
            {recordings.export.working}
          </AppText>
        </View>
      ) : null}
      <IconButton
        accessibilityLabel={player.a11y.export}
        disabled={exporting}
        icon="share"
        onPress={onExport}
        testID="player-export"
        variant="plain"
      />
    </View>
  );
}

const styles = StyleSheet.create({
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingHorizontal: 8,
    paddingVertical: 4,
    borderRadius: radius.toolbar,
    borderWidth: 1,
    borderColor: tokens.hudLine,
    backgroundColor: tokens.toolbar,
  },
  position: {
    flexShrink: 0,
  },
  working: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
});

export default PlayerControls;
