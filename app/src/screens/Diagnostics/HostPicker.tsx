import { Modal, StyleSheet, View } from 'react-native';
import BottomSheet from '../../components/BottomSheet';
import { ListGroup, ListRow } from '../../components/List';
import { diagnostics } from '../../i18n/zh/diagnostics';
import type { Computer } from '../../native/rscore/types';
import { space, tokens } from '../../theme/tokens';

export function HostPicker({
  visible,
  computers,
  selectedHostId,
  onSelect,
  onClose,
}: {
  visible: boolean;
  computers: Computer[];
  selectedHostId: string | null;
  onSelect: (computer: Computer) => void;
  onClose: () => void;
}) {
  return (
    <Modal
      animationType="fade"
      onRequestClose={onClose}
      testID="host-picker"
      transparent
      visible={visible}
    >
      <View style={styles.overlay}>
        <BottomSheet
          onClose={onClose}
          scroll
          testID="host-picker-sheet"
          title={diagnostics.connection.pickerTitle}
        >
          <ListGroup>
            {computers.map(computer => {
              const selected = computer.hostId === selectedHostId;
              const subtitle = computer.online
                ? diagnostics.connection.subtitle(
                    computer.node,
                    computer.hostVersion,
                  )
                : diagnostics.connection.offline;
              return (
                <ListRow
                  accessibilityHint={diagnostics.a11y.pickerHint(
                    subtitle,
                    selected,
                  )}
                  chevron={!selected}
                  key={computer.hostId}
                  onPress={() => onSelect(computer)}
                  right={
                    selected ? <View style={styles.selectedDot} /> : undefined
                  }
                  subtitle={subtitle}
                  testID={`host-picker-${computer.hostId}`}
                  title={computer.name}
                />
              );
            })}
          </ListGroup>
        </BottomSheet>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  overlay: {
    flex: 1,
    backgroundColor: 'transparent',
  },
  selectedDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    backgroundColor: tokens.good,
    marginRight: space.lg,
  },
});
