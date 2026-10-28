import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { pairWaiting } from '../../i18n/zh/pairWaiting';
import { pairing } from '../../i18n/zh/pairing';
import { tokens } from '../../theme/tokens';

const PERMISSIONS = ['viewOnly', 'control', 'unattended'] as const;

export function PermissionNote({ hostWord }: { hostWord: string }) {
  return (
    <View style={styles.container} testID="permission-note">
      <AppText accessibilityRole="header" color="text2" variant="bodyStrong">
        {pairWaiting.permissionTitle(hostWord)}
      </AppText>
      <View style={styles.rows}>
        {PERMISSIONS.map(permission => (
          <View key={permission} style={styles.row}>
            <View style={styles.icon}>
              <Icon
                color={permission === 'unattended' ? 'good' : 'text2'}
                name={
                  permission === 'unattended'
                    ? 'eyeOff'
                    : permission === 'control'
                    ? 'pointer'
                    : 'eye'
                }
                size={16}
              />
            </View>
            <View style={styles.copy}>
              <AppText variant="footnote" weight={600}>
                {pairing.permissions[permission].title}
              </AppText>
              <AppText color="text3" variant="caption">
                {pairing.permissions[permission].detail}
              </AppText>
            </View>
          </View>
        ))}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    gap: 12,
    padding: 16,
    borderRadius: 16,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: tokens.inputLine,
    backgroundColor: tokens.offlineCard,
  },
  rows: { gap: 10 },
  row: { flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
  icon: {
    width: 20,
    height: 20,
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: 1,
  },
  copy: { flex: 1, gap: 1 },
});

export default PermissionNote;
