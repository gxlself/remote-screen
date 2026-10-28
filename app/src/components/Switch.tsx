import { useLayoutEffect, useRef } from 'react';
import { Animated, Easing, Pressable, StyleSheet, View } from 'react-native';
import { haptic } from '../native/platform/impl/system';
import { isAndroid, material } from '../theme/platform';
import { hit, tokens } from '../theme/tokens';

const KNOB_SLIDE_MS = 160;

export type SwitchProps = {
  value: boolean;
  onValueChange: (value: boolean) => void;
  accessibilityLabel: string;
  accessibilityHint?: string;
  disabled?: boolean;
  onColor?: string;
  testID?: string;
};

export function Switch({
  value,
  onValueChange,
  accessibilityLabel,
  accessibilityHint,
  disabled = false,
  onColor,
  testID,
}: SwitchProps) {
  const android = isAndroid();
  const trackWidth = android ? material.switchTrack.width : 51;
  const trackHeight = android ? material.switchTrack.height : 31;
  const knobSize = android
    ? value
      ? material.switchThumbOn
      : material.switchThumbOff
    : 27;
  const knobLeft = android ? (value ? 24 : 8) : value ? 22 : 2;
  const trackColor = value ? onColor ?? tokens.good : tokens.switchOff;
  const slide = useRef(new Animated.Value(0)).current;
  const lastLeft = useRef(knobLeft);

  // The knob takes its new `left` at once and slides in from the old one.
  useLayoutEffect(() => {
    const from = lastLeft.current;
    lastLeft.current = knobLeft;
    if (from === knobLeft) {
      return undefined;
    }
    slide.setValue(from - knobLeft);
    const animation = Animated.timing(slide, {
      toValue: 0,
      duration: KNOB_SLIDE_MS,
      easing: Easing.out(Easing.cubic),
      useNativeDriver: true,
    });
    animation.start();
    return () => animation.stop();
  }, [knobLeft, slide]);

  return (
    <Pressable
      accessibilityLabel={accessibilityLabel}
      accessibilityHint={accessibilityHint}
      accessibilityRole="switch"
      accessibilityState={{ checked: value, disabled }}
      disabled={disabled}
      onPress={() => {
        haptic('light');
        onValueChange(!value);
      }}
      style={[
        styles.pressable,
        { width: trackWidth, height: hit.min },
        disabled ? styles.disabled : null,
      ]}
      testID={testID}
    >
      <View
        style={[
          styles.track,
          {
            width: trackWidth,
            height: trackHeight,
            backgroundColor: trackColor,
          },
          android && !value ? styles.androidTrackOff : null,
        ]}
        testID={testID == null ? undefined : `${testID}-track`}
      >
        <Animated.View
          style={[
            styles.knob,
            {
              width: knobSize,
              height: knobSize,
              borderRadius: knobSize / 2,
              top: (trackHeight - knobSize) / 2,
              left: knobLeft,
              // M3 thumb colors mapped to this theme, as specified by the task.
              backgroundColor: android && !value ? tokens.text3 : tokens.knob,
              transform: [{ translateX: slide }],
            },
          ]}
          testID={testID == null ? undefined : `${testID}-knob`}
        />
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  pressable: {
    justifyContent: 'center',
  },
  disabled: { opacity: 0.4 },
  track: {
    borderRadius: 16,
    justifyContent: 'center',
  },
  knob: {
    position: 'absolute',
  },
  androidTrackOff: { borderWidth: 2, borderColor: tokens.muted },
});
