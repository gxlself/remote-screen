import { useState } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import { formatDuration } from '../../features/recording/library';
import { recordings } from '../../i18n/zh/recordings';
import { hit, tokens } from '../../theme/tokens';

const STEP_MS = 10_000;

export type SeekBarProps = {
  positionMs: number;
  durationMs: number;
  disabled?: boolean;
  onSeek: (ms: number) => void;
};

export function SeekBar({
  positionMs,
  durationMs,
  disabled = false,
  onSeek,
}: SeekBarProps) {
  const [width, setWidth] = useState(0);
  const played =
    durationMs > 0 ? Math.min(Math.max(positionMs / durationMs, 0), 1) : 0;

  return (
    <Pressable
      accessibilityActions={[{ name: 'increment' }, { name: 'decrement' }]}
      accessibilityLabel={recordings.player.seekA11y}
      accessibilityRole="adjustable"
      accessibilityState={disabled ? { disabled: true } : undefined}
      accessibilityValue={{
        min: 0,
        max: durationMs,
        now: positionMs,
        text: recordings.player.position(
          formatDuration(positionMs),
          formatDuration(durationMs),
        ),
      }}
      disabled={disabled}
      onAccessibilityAction={event => {
        if (event.nativeEvent.actionName === 'increment') {
          onSeek(positionMs + STEP_MS);
        } else if (event.nativeEvent.actionName === 'decrement') {
          onSeek(positionMs - STEP_MS);
        }
      }}
      onLayout={event => setWidth(event.nativeEvent.layout.width)}
      onPress={event => {
        if (width > 0) {
          onSeek((event.nativeEvent.locationX / width) * durationMs);
        }
      }}
      style={[styles.bar, { minHeight: hit.min }]}
      testID="player-seekbar"
    >
      <View pointerEvents="none" style={styles.track}>
        <View
          style={[styles.played, { width: `${played * 100}%` }]}
          testID="player-seekbar-played"
        />
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  bar: {
    flex: 1,
    justifyContent: 'center',
  },
  track: {
    height: 4,
    borderRadius: 2,
    overflow: 'hidden',
    backgroundColor: tokens.track,
  },
  played: {
    height: 4,
    backgroundColor: tokens.accent,
  },
});

export default SeekBar;
