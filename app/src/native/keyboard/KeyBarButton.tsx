import type { JSX, ReactNode } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import type {
  AccessibilityActionEvent,
  AccessibilityActionInfo,
  StyleProp,
  ViewStyle,
} from 'react-native';
import AppText from '../../components/AppText';
import { hit, tokens } from '../../theme/tokens';
import type { ModState } from './keyBar';

export type KeyBarButtonVariant = 'default' | 'segment' | 'modifier' | 'plain';

export type KeyBarButtonProps = {
  label: ReactNode;
  accessibilityLabel: string;
  onPress?: () => void;
  onPressIn?: (
    pageX: number,
    pageY: number,
    locationX: number,
    locationY: number,
  ) => void;
  onPressMove?: (pageX: number, pageY: number) => void;
  onPressOut?: () => void;
  disabled?: boolean;
  selected?: boolean;
  modifierState?: ModState;
  variant?: KeyBarButtonVariant;
  style?: StyleProp<ViewStyle>;
  /** 按钮实际高度，用来补足触控区到平台最小尺寸。 */
  height?: number;
  /** false 时按下后不会被外层横向滚动抢走（方向键拖动用）。 */
  cancelable?: boolean;
  accessibilityActions?: readonly AccessibilityActionInfo[];
  onAccessibilityAction?: (event: AccessibilityActionEvent) => void;
  testID?: string;
};

const BUTTON_HEIGHT = 44;

export function KeyBarButton({
  label,
  accessibilityLabel,
  onPress,
  onPressIn,
  onPressMove,
  onPressOut,
  disabled = false,
  selected = false,
  modifierState,
  variant = 'default',
  style,
  height = BUTTON_HEIGHT,
  cancelable,
  accessibilityActions,
  onAccessibilityAction,
  testID,
}: KeyBarButtonProps): JSX.Element {
  // 44 高的键在 Android 上不到 48dp，用 hitSlop 补齐。
  const slop = Math.max(0, (hit.min - height) / 2);
  const modifier = variant === 'modifier';
  const segment = variant === 'segment';
  const locked = modifier && modifierState === 'lock';
  const once = modifier && modifierState === 'once';
  const isSelected = modifier ? modifierState !== 'none' : selected;
  const color = segment
    ? selected
      ? 'bg'
      : 'text2'
    : locked
    ? 'accentText'
    : once
    ? 'accent'
    : 'text';

  return (
    <Pressable
      accessibilityActions={accessibilityActions}
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      accessibilityState={{ selected: isSelected, disabled }}
      {...(cancelable === undefined ? {} : { cancelable })}
      disabled={disabled}
      hitSlop={slop > 0 ? { top: slop, bottom: slop } : undefined}
      onAccessibilityAction={onAccessibilityAction}
      onPress={onPress}
      onPressIn={event =>
        onPressIn?.(
          event.nativeEvent.pageX,
          event.nativeEvent.pageY,
          event.nativeEvent.locationX,
          event.nativeEvent.locationY,
        )
      }
      onPressMove={event =>
        onPressMove?.(event.nativeEvent.pageX, event.nativeEvent.pageY)
      }
      onPressOut={onPressOut}
      style={({ pressed }) => [
        styles.button,
        variant === 'default' ? styles.default : null,
        variant === 'plain' ? styles.plain : null,
        segment
          ? selected
            ? styles.segmentSelected
            : styles.segmentIdle
          : null,
        modifier && !once && !locked ? styles.modifierIdle : null,
        once ? styles.modifierOnce : null,
        locked ? styles.modifierLocked : null,
        disabled ? styles.disabled : null,
        pressed && !disabled
          ? pressedStyle(variant, (segment && selected) || locked)
          : null,
        style,
      ]}
      testID={testID}
    >
      {typeof label === 'string' ? (
        <AppText
          color={color}
          numberOfLines={1}
          style={styles.label}
          variant={segment ? 'caption' : 'footnote'}
          weight={500}
        >
          {label}
        </AppText>
      ) : (
        label
      )}
      {locked ? (
        <View
          accessibilityElementsHidden
          importantForAccessibility="no-hide-descendants"
          style={styles.lockMark}
        />
      ) : null}
    </Pressable>
  );
}

function pressedStyle(
  variant: KeyBarButtonVariant,
  filled: boolean,
): ViewStyle {
  // 已选中或已锁定的键底色是强调色，按下时只压暗，不换颜色。
  if (filled) {
    return styles.pressedDim;
  }
  if (variant === 'segment') {
    return styles.pressedSegment;
  }
  return variant === 'plain' ? styles.pressedPlain : styles.pressed;
}

const styles = StyleSheet.create({
  button: {
    minWidth: 44,
    height: 44,
    paddingHorizontal: 10,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
  },
  label: { textAlign: 'center' },
  default: { backgroundColor: '#232A30' },
  plain: { backgroundColor: 'transparent' },
  segmentIdle: { backgroundColor: 'transparent' },
  segmentSelected: { backgroundColor: tokens.text },
  modifierIdle: { backgroundColor: '#232A30' },
  modifierOnce: {
    backgroundColor: 'rgba(125, 168, 255, 0.14)',
    borderWidth: 1,
    borderColor: tokens.accent,
  },
  modifierLocked: { backgroundColor: tokens.accent },
  lockMark: {
    position: 'absolute',
    bottom: 5,
    width: 12,
    height: 2,
    borderRadius: 1,
    backgroundColor: tokens.accentText,
  },
  disabled: { opacity: 0.4 },
  // 按下时变亮一档，松手恢复。
  pressed: { backgroundColor: '#39434B' },
  pressedPlain: { backgroundColor: 'rgba(255,255,255,0.08)' },
  pressedSegment: { backgroundColor: 'rgba(255,255,255,0.08)' },
  pressedDim: { opacity: 0.75 },
});

export default KeyBarButton;
