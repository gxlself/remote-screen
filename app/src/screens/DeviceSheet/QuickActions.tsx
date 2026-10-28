import {
  PixelRatio,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import AppText from '../../components/AppText';
import { Icon, type IconName } from '../../components/icons/Icon';
import { deviceSheet } from '../../i18n/zh/deviceSheet';
import { isAndroid, press } from '../../theme/platform';
import { tokens } from '../../theme/tokens';

type QuickActionProps = {
  label: string;
  icon: IconName;
  onPress: () => void;
  disabled?: boolean;
  /** Looks unavailable but still answers a tap, with the reason. */
  dimmed?: boolean;
  hint?: string;
  testID: string;
  largeFont?: boolean;
};

function QuickAction({
  label,
  icon,
  onPress,
  disabled = false,
  dimmed = false,
  hint,
  testID,
  largeFont = false,
}: QuickActionProps) {
  const faded = disabled || dimmed;
  // 同 ripple.neutral。
  const rippleProps =
    Platform.OS === 'android'
      ? { android_ripple: { color: 'rgba(255, 255, 255, 0.12)' } }
      : null;

  return (
    <Pressable
      accessibilityHint={hint}
      accessibilityLabel={label}
      accessibilityRole="button"
      // A dimmed action still answers a tap with its reason, but screen
      // readers hear it as unavailable like a disabled one.
      accessibilityState={faded ? { disabled: true } : undefined}
      {...rippleProps}
      // `disabled={false}` would overwrite the dimmed state above.
      disabled={disabled ? true : undefined}
      onPress={onPress}
      style={({ pressed }) => [
        styles.action,
        faded ? styles.disabled : null,
        pressed && !faded && !isAndroid() ? styles.pressed : null,
        largeFont ? styles.actionLargeFont : null,
      ]}
      testID={testID}
    >
      <Icon color={faded ? 'text3' : 'text2'} name={icon} size={20} />
      <AppText color={faded ? 'text3' : 'text'} variant="footnote">
        {label}
      </AppText>
    </Pressable>
  );
}

export type QuickActionsProps = {
  onViewOnly: () => void;
  onTerminal: () => void;
  onLock: () => void;
  canLock: boolean;
  /** Says why locking is unavailable (no session yet). */
  onLockUnavailable?: () => void;
  /** The computer is offline: nothing here can reach it. */
  offline?: boolean;
};

// 传文件 lives in the 文件 tab, which cannot preselect a computer yet, so it
// is not offered here.
export function QuickActions({
  onViewOnly,
  onTerminal,
  onLock,
  canLock,
  onLockUnavailable,
  offline = false,
}: QuickActionsProps) {
  const largeFont = PixelRatio.getFontScale() > 2;

  return (
    <View
      style={[styles.row, largeFont ? styles.rowLargeFont : null]}
      testID="quick-actions"
    >
      <QuickAction
        disabled={offline}
        hint={offline ? deviceSheet.quickActions.offlineHint : undefined}
        icon="eye"
        label={deviceSheet.quickActions.viewOnly}
        largeFont={largeFont}
        onPress={onViewOnly}
        testID="quick-action-view-only"
      />
      <QuickAction
        disabled={offline}
        hint={offline ? deviceSheet.quickActions.offlineHint : undefined}
        icon="terminal"
        label={deviceSheet.quickActions.terminal}
        largeFont={largeFont}
        onPress={onTerminal}
        testID="quick-action-terminal"
      />
      <QuickAction
        dimmed={!canLock}
        hint={canLock ? undefined : deviceSheet.a11y.lockNeedsSession}
        icon="lock"
        label={deviceSheet.quickActions.lock}
        largeFont={largeFont}
        onPress={canLock ? onLock : () => onLockUnavailable?.()}
        testID="quick-action-lock"
      />
    </View>
  );
}

const styles = StyleSheet.create({
  row: {
    flexDirection: 'row',
    gap: 8,
  },
  rowLargeFont: {
    flexWrap: 'wrap',
  },
  action: {
    flex: 1,
    minHeight: 64,
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    paddingHorizontal: 4,
    paddingVertical: 8,
    borderRadius: 12,
    backgroundColor: tokens.raised,
  },
  actionLargeFont: {
    flexBasis: '47%',
  },
  disabled: {
    opacity: 0.4,
  },
  pressed: {
    opacity: press.opacity,
  },
});

export default QuickActions;
