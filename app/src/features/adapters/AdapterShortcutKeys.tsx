import type { JSX } from 'react';
import { PixelRatio, Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { ipadSession } from '../../i18n/zh/ipadSession';
import type { SessionCombo } from '../../native/rscore/types';
import { radius } from '../../theme/tokens';
import { ipadKeysFor } from '../session/ipadKeys';

export type AdapterShortcutKeysProps = {
  os: 'macos' | 'windows';
  /** Laid out along the toolbar. */
  vertical: boolean;
  onCombo(combo: SessionCombo): void;
  onHint(label: string): void;
};

/**
 * V41: the shortcuts iPadOS keeps for itself, as on the Host toolbar. Keys
 * the adapters cannot send (the globe key) are left out.
 */
export function AdapterShortcutKeys({
  os,
  vertical,
  onCombo,
  onHint,
}: AdapterShortcutKeysProps): JSX.Element {
  const hint =
    os === 'windows'
      ? ipadSession.shortcutsLabel.windows
      : ipadSession.shortcutsLabel.mac;

  return (
    <View
      accessibilityLabel={hint}
      style={[styles.group, vertical ? styles.column : styles.row]}
    >
      {ipadKeysFor(os).map(key => {
        const combo = key.combo;
        if (combo === null) {
          return null;
        }
        return (
          <Pressable
            accessibilityHint={hint}
            accessibilityLabel={key.a11y}
            accessibilityRole="button"
            key={key.id}
            onLongPress={() => onHint(key.a11y)}
            onPress={() => onCombo(combo)}
            style={({ pressed }) => [
              styles.key,
              PixelRatio.getFontScale() > 2
                ? { height: undefined, minHeight: 44 }
                : null,
              pressed ? styles.pressed : null,
            ]}
            testID={`adapter-key-${key.id}`}
          >
            <AppText color="text" mono variant="mono">
              {key.label}
            </AppText>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  group: { alignItems: 'center', gap: 4 },
  row: { flexDirection: 'row' },
  column: { flexDirection: 'column' },
  key: {
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 10,
    borderRadius: radius.panel,
  },
  pressed: { opacity: 0.8 },
});

export default AdapterShortcutKeys;
