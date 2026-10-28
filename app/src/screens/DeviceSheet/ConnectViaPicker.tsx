import { Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import Dialog from '../../components/Dialog';
import { Icon } from '../../components/icons/Icon';
import type { ConnectVia } from '../../features/native/types';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import { isAndroid, ripple } from '../../theme/platform';
import { tokens } from '../../theme/tokens';

const OPTIONS: readonly ConnectVia[] = ['auto', 'native', 'host'];

/** `auto` follows the global `connect.prefer`, so its label says which goes first. */
export function connectViaLabel(
  via: ConnectVia,
  os: 'macos' | 'windows',
  prefer: 'native' | 'host',
): string {
  switch (via) {
    case 'auto':
      return prefer === 'host'
        ? native.connectVia.autoHost
        : native.connectVia.auto;
    case 'native':
      return native.connectVia.native(os);
    case 'host':
      return native.connectVia.host;
  }
}

export type ConnectViaPickerProps = {
  visible: boolean;
  os: 'macos' | 'windows';
  prefer: 'native' | 'host';
  value: ConnectVia;
  onChange(via: ConnectVia): void;
  onClose(): void;
};

export function ConnectViaPicker({
  visible,
  os,
  prefer,
  value,
  onChange,
  onClose,
}: ConnectViaPickerProps) {
  const android = isAndroid();

  return (
    <Dialog
      actions={[{ label: common.cancel, onPress: onClose, variant: 'secondary' }]}
      onDismiss={onClose}
      testID="connect-via-dialog"
      title={native.connectVia.title}
      visible={visible}
    >
      <View
        accessibilityLabel={native.connectVia.title}
        accessibilityRole="radiogroup"
        style={styles.options}
      >
        {OPTIONS.map(option => {
          const selected = option === value;
          const label = connectViaLabel(option, os, prefer);
          return (
            <Pressable
              accessibilityLabel={label}
              accessibilityRole="radio"
              accessibilityState={{ checked: selected, selected }}
              android_ripple={android ? { color: ripple.neutral } : undefined}
              key={option}
              onPress={() => onChange(option)}
              style={[styles.option, selected ? styles.optionSelected : null]}
              testID={`connect-via-${option}`}
            >
              <AppText style={styles.label} variant="body">
                {label}
              </AppText>
              {selected ? <Icon color="accent" name="check" size={18} /> : null}
            </Pressable>
          );
        })}
      </View>
    </Dialog>
  );
}

const styles = StyleSheet.create({
  options: {
    alignSelf: 'stretch',
    gap: 8,
  },
  option: {
    minHeight: 52,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 14,
    borderWidth: 2,
    borderRadius: 14,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.raised,
  },
  optionSelected: {
    borderColor: tokens.accent,
  },
  label: {
    flex: 1,
  },
});
