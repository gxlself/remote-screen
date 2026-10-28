import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { sessions } from '../../i18n/zh/sessions';
import type { SshSessionItem } from './useSessionsModel';

const TERMINAL_TILE = '#12241B';

export type LiveSshListProps = {
  items: SshSessionItem[];
  onOpen: (item: SshSessionItem) => void;
};

export default function LiveSshList({ items, onOpen }: LiveSshListProps) {
  return (
    <ListGroup>
      {items.map(item => (
        <View key={item.sshId} style={styles.row}>
          <ListRow
            accessibilityLabel={sessions.ssh.rowA11y(item.name, item.subtitle)}
            left={
              <View style={styles.icon}>
                <Icon color="good" name="terminal" size={19} />
              </View>
            }
            onPress={() => onOpen(item)}
            right={
              <AppText color="text2" mono variant="monoSmall">
                {item.elapsed}
              </AppText>
            }
            subtitle={item.subtitle}
            testID={`ssh-row-${item.sshId}`}
            title={item.name}
          />
        </View>
      ))}
    </ListGroup>
  );
}

const styles = StyleSheet.create({
  row: {
    minHeight: 64,
    justifyContent: 'center',
  },
  icon: {
    width: 36,
    height: 36,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 10,
    backgroundColor: TERMINAL_TILE,
  },
});
