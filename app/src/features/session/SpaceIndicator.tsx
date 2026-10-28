import { useEffect, useRef } from 'react';
import type { JSX } from 'react';
import { Animated, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { spaces as copy } from '../../i18n/zh/spaces';
import type { SpaceTile } from '../../native/session/RemoteSessionView';
import { tokens } from '../../theme/tokens';
import { SPACE_FADE_MS } from './spaces';

export type SpaceIndicatorProps = {
  spaces: readonly SpaceTile[];
  highlightId: string;
  fading: boolean;
  reduceMotion: boolean;
};

/**
 * one dot per connected computer and the name of the one in
 * front, at the top while switching and for a moment after, then it fades.
 */
export function SpaceIndicator({
  spaces,
  highlightId,
  fading,
  reduceMotion,
}: SpaceIndicatorProps): JSX.Element | null {
  const insets = useSafeAreaInsets();
  const opacity = useRef(new Animated.Value(1)).current;
  useEffect(() => {
    const animation = Animated.timing(opacity, {
      toValue: fading ? 0 : 1,
      duration: reduceMotion ? 0 : SPACE_FADE_MS,
      useNativeDriver: true,
    });
    animation.start();
    return () => animation.stop();
  }, [fading, opacity, reduceMotion]);

  const index = spaces.findIndex(space => space.id === highlightId);
  if (index < 0) {
    return null;
  }
  const name = spaces[index].name;
  return (
    <View
      pointerEvents="none"
      style={[styles.layer, { top: insets.top + 8 }]}
      testID="space-indicator-layer"
    >
      <Animated.View
        accessibilityLabel={copy.a11y(index + 1, spaces.length, name)}
        accessibilityLiveRegion="polite"
        accessible
        style={[styles.pill, { opacity }]}
        testID="space-indicator"
      >
        <View style={styles.dots}>
          {spaces.map((space, at) => (
            <View
              accessibilityState={{ selected: at === index }}
              key={space.id}
              style={[styles.dot, at === index ? styles.current : null]}
              testID={`space-dot-${at}`}
            />
          ))}
        </View>
        <AppText
          color="text"
          numberOfLines={1}
          testID="space-indicator-name"
          variant="footnote"
          weight={500}
        >
          {name}
        </AppText>
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  layer: {
    position: 'absolute',
    left: 0,
    right: 0,
    alignItems: 'center',
  },
  pill: {
    alignItems: 'center',
    gap: 4,
    maxWidth: 320,
    paddingHorizontal: 14,
    paddingVertical: 6,
    borderRadius: 14,
    backgroundColor: tokens.hud,
  },
  dots: { flexDirection: 'row', gap: 6 },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: tokens.muted,
  },
  current: { backgroundColor: tokens.text },
});

export default SpaceIndicator;
