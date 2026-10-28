import { Pressable, StyleSheet, View } from 'react-native';
import { boxHeight } from '../theme/fontScale';
import { isAndroid, press, ripple } from '../theme/platform';
import { hit, tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon, type IconName } from './icons/Icon';

export type IconButtonVariant = 'raised' | 'accentSoft' | 'plain';

export type IconButtonProps = {
  icon: IconName;
  accessibilityLabel: string;
  onPress: () => void;
  onLongPress?: () => void;
  variant?: IconButtonVariant;
  size?: 44 | 36;
  disabled?: boolean;
  badge?: number;
  testID?: string;
};

const variantBackgrounds: Record<IconButtonVariant, string | undefined> = {
  raised: tokens.raised,
  accentSoft: tokens.accentSoft,
  plain: undefined,
};

const variantIconColors: Record<IconButtonVariant, keyof typeof tokens> = {
  raised: 'text',
  accentSoft: 'accent',
  plain: 'text',
};

export function IconButton({
  icon,
  accessibilityLabel,
  onPress,
  onLongPress,
  variant = 'raised',
  size = 44,
  disabled = false,
  badge,
  testID,
}: IconButtonProps) {
  const android = isAndroid();
  const showBadge = badge != null && badge > 0;
  const hitSlop = size < hit.min ? (hit.min - size) / 2 : undefined;

  return (
    <Pressable
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      accessibilityState={disabled ? { disabled: true } : undefined}
      android_ripple={
        android
          ? { color: ripple.neutral, borderless: true, radius: size / 2 + 4 }
          : undefined
      }
      disabled={disabled}
      hitSlop={hitSlop}
      onLongPress={onLongPress}
      onPress={onPress}
      style={({ pressed }) => [
        styles.button,
        {
          width: size,
          height: size,
          borderRadius: size / 2,
          backgroundColor: variantBackgrounds[variant],
        },
        pressed && !android ? styles.pressed : null,
        disabled ? styles.disabled : null,
      ]}
      testID={testID}
    >
      <Icon color={variantIconColors[variant]} name={icon} size={22} />
      {showBadge ? (
        <View
          style={[styles.badge, boxHeight(18)]}
          testID={testID == null ? undefined : `${testID}-badge`}
        >
          <AppText color="accentText" variant="tiny" weight={700}>
            {String(badge)}
          </AppText>
        </View>
      ) : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  pressed: { opacity: press.opacity },
  disabled: { opacity: 0.4 },
  badge: {
    position: 'absolute',
    top: 0,
    right: 0,
    minWidth: 18,
    borderRadius: 9,
    paddingHorizontal: 4,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accent,
  },
});
