import type { JSX } from 'react';
import { PixelRatio, Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ipadKeysFor } from '../../features/session/ipadKeys';
import type { IpadKey } from '../../features/session/ipadKeys';
import { useSetting } from '../../features/settings/store';
import { ipadSession } from '../../i18n/zh/ipadSession';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { hit, radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';

function Separator() {
  return (
    <View
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      style={styles.divider}
    />
  );
}

export function IpadShortcutKeys({ ctx }: SlotProps): JSX.Element | null {
  const core = useCore();
  const platform = usePlatform();
  const [haptics] = useSetting('session.haptics');

  if (!ctx.isPad) {
    return null;
  }

  const keyboardEnabled = ctx.view.toolbar.keyboardEnabled;
  const hasSession = ctx.sessionId !== null;
  const shortcutHint =
    ctx.os === 'windows'
      ? ipadSession.shortcutsLabel.windows
      : ipadSession.shortcutsLabel.mac;
  const hitPadding = Math.max(0, (hit.min - 44) / 2);

  const press = (key: IpadKey) => {
    if (ctx.sessionId === null || key.combo === null || !keyboardEnabled) {
      return;
    }
    core.sessions.combo(ctx.sessionId, key.combo);
    if (haptics) {
      platform.haptic('light');
    }
  };

  return (
    <View accessibilityLabel={shortcutHint} style={styles.group}>
      <Separator />
      {ipadKeysFor(ctx.os).map(key => {
        const disabled = key.combo === null || !keyboardEnabled || !hasSession;
        const suffix = `${
          key.combo === null ? ipadSession.unsupportedSuffix : ''
        }${!keyboardEnabled ? ipadSession.viewerSuffix : ''}`;

        return (
          <Pressable
            accessibilityHint={shortcutHint}
            accessibilityLabel={`${key.a11y}${suffix}`}
            accessibilityRole="button"
            accessibilityState={disabled ? { disabled: true } : undefined}
            disabled={disabled}
            hitSlop={{ top: hitPadding, bottom: hitPadding }}
            key={key.id}
            onPress={() => press(key)}
            style={({ pressed }) => [
              styles.key,
              PixelRatio.getFontScale() > 2
                ? { height: undefined, minHeight: 44 }
                : null,
              disabled ? styles.disabled : null,
              pressed ? styles.pressed : null,
            ]}
            testID={`ipad-key-${key.id}`}
          >
            {key.id === 'globe' ? (
              <Icon color="text" name="globe" size={18} testID="ipad-globe" />
            ) : (
              <AppText color="text" mono variant="mono">
                {key.label}
              </AppText>
            )}
          </Pressable>
        );
      })}
      <Separator />
    </View>
  );
}

const styles = StyleSheet.create({
  group: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    marginHorizontal: 4,
  },
  divider: {
    width: 1,
    height: 24,
    marginHorizontal: 4,
    backgroundColor: tokens.inputLine,
  },
  key: {
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 10,
    borderRadius: radius.panel,
  },
  disabled: { opacity: 0.4 },
  pressed: { opacity: 0.8 },
});

export default IpadShortcutKeys;
