import type { ReactNode } from 'react';
import { Platform, Pressable, StyleSheet, View } from 'react-native';
import { tokens, space, radius } from '../../theme/tokens';
import AppText from '../../components/AppText';

export type ModeCardProps = {
  title: string;
  description: string;
  selected: boolean;
  disabled?: boolean;
  disabledHint?: string;
  status?: ReactNode;
  onPress: () => void;
  accessibilityLabel?: string;
  testID?: string;
};

export function ModeCard({
  title,
  description,
  selected,
  disabled = false,
  disabledHint,
  status,
  onPress,
  accessibilityLabel,
  testID,
}: ModeCardProps) {
  const cardAccessibilityLabel = accessibilityLabel ?? title;
  let radioState: {
    selected: boolean;
    checked: boolean;
    disabled?: boolean;
  } = {
    selected,
    checked: selected,
  };
  if (disabled) {
    radioState = { selected, checked: selected, disabled: true };
  }

  const android = Platform.OS === 'android';

  return (
    <Pressable
      accessibilityHint={disabled ? disabledHint : description}
      accessibilityLabel={cardAccessibilityLabel}
      accessibilityRole="radio"
      accessibilityState={radioState}
      // 同 ripple.neutral
      android_ripple={
        android ? { color: 'rgba(255, 255, 255, 0.12)' } : undefined
      }
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [
        styles.card,
        selected ? styles.selected : styles.unselected,
        disabled ? styles.disabled : null,
        pressed && !disabled && !android ? styles.pressed : null,
      ]}
      testID={testID}
    >
      <View style={styles.body}>
        <View style={styles.titleRow}>
          <View style={styles.titleBlock}>
            <AppText variant="bodyStrong" weight={600}>
              {title}
            </AppText>
          </View>
          {status == null ? null : <View>{status}</View>}
        </View>
        <AppText color="text2" variant="footnote" style={styles.description}>
          {description}
        </AppText>
        {disabled && disabledHint !== undefined ? (
          <AppText
            color="text3"
            testID={testID === undefined ? undefined : `${testID}-disabled`}
            variant="caption"
          >
            {disabledHint}
          </AppText>
        ) : null}
      </View>
      <View style={[styles.radio, selected ? styles.radioSelected : null]}>
        {selected ? <View style={styles.dot} /> : null}
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  card: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: space.md,
    minHeight: 76,
    padding: space.lg,
    borderRadius: radius.card,
  },
  selected: {
    backgroundColor: tokens.accentTint,
    borderWidth: 1.5,
    borderColor: tokens.accent,
  },
  unselected: {
    backgroundColor: tokens.card,
    borderWidth: 1.5,
    borderColor: tokens.inputLine,
  },
  disabled: {
    opacity: 0.55,
  },
  pressed: {
    opacity: 0.8,
  },
  body: {
    flex: 1,
    gap: 4,
    minWidth: 0,
  },
  titleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: space.sm,
  },
  titleBlock: {
    flexShrink: 1,
  },
  description: {
    marginTop: 2,
  },
  radio: {
    width: 20,
    height: 20,
    borderRadius: 10,
    borderWidth: 1.5,
    borderColor: tokens.inputLine,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: 1,
  },
  radioSelected: {
    borderColor: tokens.accent,
  },
  dot: {
    width: 10,
    height: 10,
    borderRadius: 5,
    backgroundColor: tokens.accent,
  },
});

export default ModeCard;
