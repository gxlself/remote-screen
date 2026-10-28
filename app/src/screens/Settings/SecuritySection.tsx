import { StyleSheet, View } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { usePairedHosts, useSshKeys } from '../../features/data/queries';
import { useAuthName } from '../../features/lock/authName';
import { useSetting } from '../../features/settings/store';
import { settings as copy } from '../../i18n/zh/settings';
import { space } from '../../theme/tokens';
import { useRecordingsSettingsRow } from '../../features/recording/RecordingsSettingsRow';

type Props = {
  navigation: TabScreenProps<'Settings'>['navigation'];
};

export function SecuritySection({ navigation }: Props) {
  const authenticationName = useAuthName();
  const [onOpen] = useSetting('lock.onOpen');
  const [beforeSession] = useSetting('lock.beforeSession');
  const pairedHosts = usePairedHosts();
  const keys = useSshKeys();
  const lockEnabled = onOpen || beforeSession;
  const recordingsRow = useRecordingsSettingsRow(navigation);

  return (
    <View style={styles.section}>
      <SectionHeader title={copy.security.section} />
      <ListGroup>
        <ListRow
          chevron
          onPress={() => navigation.navigate('AppLock')}
          subtitle={copy.security.faceId.subtitle}
          title={copy.security.faceId.title(authenticationName)}
          value={
            lockEnabled
              ? copy.security.faceId.valueOn
              : copy.security.faceId.valueOff
          }
        />
        {/* S16: the list itself lives on the Home tab. */}
        <ListRow
          accessibilityHint={copy.security.pairedHosts.hint}
          chevron
          onPress={() => navigation.navigate('Home')}
          testID="settings-paired-hosts"
          title={copy.security.pairedHosts.title}
          value={copy.security.pairedHosts.count(pairedHosts.data?.length ?? 0)}
        />
        {/* S20: always on, so it reads as a fact rather than a choice. */}
        <ListRow
          subtitle={copy.security.fingerprint.subtitle}
          title={copy.security.fingerprint.title}
        />
        <ListRow
          chevron
          onPress={() => navigation.navigate('SshKeys')}
          subtitle={copy.security.sshKeys.subtitle}
          title={copy.security.sshKeys.title}
          value={copy.security.sshKeys.count(keys.data?.length ?? 0)}
        />
        {recordingsRow}
      </ListGroup>
    </View>
  );
}

const styles = StyleSheet.create({
  section: {
    gap: space.sm,
  },
});

export default SecuritySection;
