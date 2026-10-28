import type { JSX, ReactNode } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import type { StyleProp, ViewStyle } from 'react-native';
import AppText from '../../components/AppText';
import { isAndroid, press, ripple } from '../../theme/platform';
import { hit, tokens } from '../../theme/tokens';
import type { ModState } from './termKeys';

export type TermKeyButtonProps = {
  label: ReactNode;
  accessibilityLabel: string;
  onPress?: () => void;
  disabled?: boolean;
  modifierState?: ModState;
  variant?: 'key' | 'toggle' | 'toggleOpen';
  glyphSize?: number;
  style?: StyleProp<ViewStyle>;
  testID?: string;
};

export function TermKeyButton({
  label,
  accessibilityLabel,
  onPress,
  disabled = false,
  modifierState,
  variant = 'key',
  glyphSize,
  style,
  testID,
}: TermKeyButtonProps): JSX.Element {
  const android = isAndroid();
  const modifier = modifierState !== undefined;
  const once = modifierState === 'once';
  const locked = modifierState === 'lock';
  const glyphColor = locked
    ? 'accentText'
    : once
    ? 'accent'
    : variant === 'toggle'
    ? 'accentText'
    : 'text';

  return (
    <Pressable
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      accessibilityState={{
        disabled,
        selected: modifier ? modifierState !== 'none' : false,
      }}
      android_ripple={android ? { color: ripple.neutral } : undefined}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [
        styles.button,
        // 44 pt on iOS, the 48 dp touch target on Android.
        { height: hit.min },
        variant === 'toggle' ? styles.toggle : null,
        variant === 'toggleOpen' ? styles.toggleOpen : null,
        modifier && !once && !locked ? styles.modifierIdle : null,
        once ? styles.modifierOnce : null,
        locked ? styles.modifierLocked : null,
        pressed && !android ? styles.pressed : null,
        disabled ? styles.disabled : null,
        style,
      ]}
      testID={testID}
    >
      {typeof label === 'string' ? (
        <AppText
          color={glyphColor}
          mono
          numberOfLines={1}
          style={[
            styles.label,
            glyphSize === undefined ? null : { fontSize: glyphSize },
          ]}
        >
          {label}
        </AppText>
      ) : (
        label
      )}
      {locked ? <View style={styles.lockMark} /> : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: {
    borderRadius: 12,
    overflow: 'hidden',
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  toggle: { backgroundColor: tokens.accent },
  toggleOpen: { backgroundColor: tokens.selected },
  modifierIdle: { backgroundColor: tokens.raised },
  modifierOnce: {
    backgroundColor: 'rgba(125, 168, 255, 0.14)',
    borderWidth: 1,
    borderColor: tokens.accent,
  },
  modifierLocked: { backgroundColor: tokens.accent },
  label: { fontSize: 13, lineHeight: 18, textAlign: 'center' },
  lockMark: {
    position: 'absolute',
    bottom: 5,
    width: 14,
    height: 2,
    borderRadius: 1,
    backgroundColor: tokens.accentText,
  },
  pressed: { opacity: press.opacity },
  disabled: { opacity: 0.4 },
});
