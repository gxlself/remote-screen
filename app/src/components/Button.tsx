import { ActivityIndicator, Pressable, StyleSheet } from 'react-native';
import type { AccessibilityState } from 'react-native';
import { boxHeight } from '../theme/fontScale';
import { isAndroid, ripple } from '../theme/platform';
import { hit, tokens } from '../theme/tokens';
import type { TypeVariantName, Weight } from '../theme/typography';
import AppText from './AppText';
import { Icon, type IconName } from './icons/Icon';

export type ButtonVariant =
  | 'primaryLarge'
  | 'primary'
  | 'secondary'
  | 'pill'
  | 'pillDanger'
  | 'text';

// alignSelf. Without it the button follows its parent's alignItems; in a
// plain column that stretches it, so pass 'start' to keep it hugging its title.
export type ButtonAlign = 'start' | 'center' | 'end' | 'stretch';

export type ButtonProps = {
  title: string;
  onPress: () => void;
  variant?: ButtonVariant;
  icon?: IconName;
  disabled?: boolean;
  loading?: boolean;
  fullWidth?: boolean;
  align?: ButtonAlign;
  accessibilityLabel?: string;
  accessibilityHint?: string;
  textColor?: keyof typeof tokens;
  testID?: string;
};

type VariantSpec = {
  height: number;
  borderRadius: number;
  backgroundColor: string;
  color: keyof typeof tokens;
  textVariant: TypeVariantName;
  textWeight: Weight;
  paddingHorizontal: number;
};

const variantSpecs = {
  primaryLarge: {
    height: 54,
    borderRadius: 16,
    backgroundColor: tokens.accent,
    color: 'accentText',
    textVariant: 'headline',
    textWeight: 600,
    paddingHorizontal: 24,
  },
  primary: {
    height: 44,
    borderRadius: 12,
    backgroundColor: tokens.accent,
    color: 'accentText',
    textVariant: 'button',
    textWeight: 600,
    paddingHorizontal: 16,
  },
  secondary: {
    height: 44,
    borderRadius: 12,
    backgroundColor: tokens.raised,
    color: 'text',
    textVariant: 'button',
    textWeight: 400,
    paddingHorizontal: 16,
  },
  pill: {
    height: 44,
    borderRadius: 22,
    backgroundColor: tokens.accentSoft,
    color: 'accent',
    textVariant: 'button',
    textWeight: 600,
    paddingHorizontal: 16,
  },
  pillDanger: {
    height: 44,
    borderRadius: 22,
    backgroundColor: tokens.badSoft,
    color: 'bad',
    textVariant: 'button',
    textWeight: 600,
    paddingHorizontal: 16,
  },
  text: {
    height: 44,
    borderRadius: 0,
    backgroundColor: 'transparent',
    color: 'accent',
    textVariant: 'body',
    textWeight: 500,
    paddingHorizontal: 16,
  },
} as const satisfies Record<ButtonVariant, VariantSpec>;

export function Button({
  title,
  onPress,
  variant = 'primary',
  icon,
  disabled = false,
  loading = false,
  fullWidth = false,
  align,
  accessibilityLabel,
  accessibilityHint,
  textColor,
  testID,
}: ButtonProps) {
  const android = isAndroid();
  const spec = variantSpecs[variant];
  const h = android ? Math.max(spec.height, hit.min) : spec.height;
  const rippleColor =
    variant === 'pill'
      ? ripple.accent
      : variant === 'pillDanger'
      ? ripple.danger
      : ripple.neutral;
  const color = tokens[textColor ?? spec.color];
  const alignSelf = fullWidth ? 'stretch' : align;
  const accessibilityState: AccessibilityState = {};
  if (loading) {
    accessibilityState.busy = true;
  }
  if (disabled) {
    accessibilityState.disabled = true;
  }
  const hasAccessibilityState = Object.keys(accessibilityState).length > 0;

  return (
    <Pressable
      accessibilityHint={accessibilityHint}
      accessibilityLabel={accessibilityLabel ?? title}
      accessibilityRole="button"
      accessibilityState={
        hasAccessibilityState ? accessibilityState : undefined
      }
      android_ripple={
        android ? { color: rippleColor, foreground: true } : undefined
      }
      disabled={disabled}
      onPress={() => {
        if (!loading) {
          onPress();
        }
      }}
      style={({ pressed }) => [
        styles.button,
        {
          ...boxHeight(h),
          borderRadius: spec.borderRadius,
          backgroundColor: spec.backgroundColor,
          paddingHorizontal: spec.paddingHorizontal,
        },
        android ? { minWidth: hit.min } : null,
        alignSelf == null ? null : alignStyles[alignSelf],
        pressed && !android ? styles.pressed : null,
        disabled ? styles.disabled : null,
      ]}
      testID={testID}
    >
      {loading ? (
        <ActivityIndicator
          color={color}
          size="small"
          testID={testID == null ? undefined : `${testID}-spinner`}
        />
      ) : (
        <>
          {icon == null ? null : (
            <Icon
              color={color}
              name={icon}
              size={18}
              testID={testID == null ? undefined : `${testID}-icon`}
            />
          )}
          <AppText
            color={spec.color}
            variant={spec.textVariant}
            weight={spec.textWeight}
          >
            {title}
          </AppText>
        </>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
  },
  pressed: { opacity: 0.8 },
  disabled: { opacity: 0.4 },
});

const alignStyles = StyleSheet.create({
  start: { alignSelf: 'flex-start' },
  center: { alignSelf: 'center' },
  end: { alignSelf: 'flex-end' },
  stretch: { alignSelf: 'stretch' },
});
