import { ActivityIndicator, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import type { IconName } from '../../components/icons/Icon';
import { tokens } from '../../theme/tokens';
import type { HistoryItem } from './useSessionsModel';

export type HistoryListProps = {
  items: HistoryItem[];
  onOpen: (item: HistoryItem) => void;
  /** The entry being reconnected; the others wait for it. */
  busyId?: string | null;
};

const kindIcon: Record<HistoryItem['kind'], IconName> = {
  remote: 'computer',
  ssh: 'terminal',
  adapter: 'computer',
};

export default function HistoryList({
  items,
  onOpen,
  busyId = null,
}: HistoryListProps) {
  return (
    <ListGroup>
      {items.map(item => {
        const busy = busyId === item.id;
        return (
          <ListRow
            accessibilityLabel={item.a11y}
            accessibilityState={busy ? { busy: true } : undefined}
            disabled={busyId !== null && !busy}
            key={item.id}
            left={
              <View style={styles.icon}>
                <Icon color="text2" name={kindIcon[item.kind]} size={17} />
              </View>
            }
            onPress={() => {
              if (!busy) {
                onOpen(item);
              }
            }}
            right={
              busy ? (
                <ActivityIndicator
                  color={tokens.text2}
                  testID={`history-busy-${item.id}`}
                />
              ) : (
                <AppText color="text3" variant="caption">
                  {item.when}
                </AppText>
              )
            }
            subtitle={item.subtitle}
            testID={`history-row-${item.id}`}
            title={item.title}
          />
        );
      })}
    </ListGroup>
  );
}

const styles = StyleSheet.create({
  icon: {
    width: 32,
    height: 32,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 8,
    backgroundColor: tokens.raised,
  },
});
