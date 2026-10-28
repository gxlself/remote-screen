import { Modal, Pressable, StyleSheet } from 'react-native';
import { BottomSheet } from '../../components/BottomSheet';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { isAndroid, press, ripple } from '../../theme/platform';

type Option<T extends string | number> = {
  value: T;
  label: string;
  subtitle?: string;
};

type Props<T extends string | number> = {
  title: string;
  options: Option<T>[];
  value: T;
  onSelect: (value: T) => void;
  onClose: () => void;
};

export function OptionSheet<T extends string | number>({
  title,
  options,
  value,
  onSelect,
  onClose,
}: Props<T>) {
  const android = isAndroid();
  return (
    <Modal animationType="none" onRequestClose={onClose} transparent visible>
      <BottomSheet onClose={onClose} title={title}>
        <ListGroup>
          {options.map(option => {
            const selected = option.value === value;
            return (
              <Pressable
                accessibilityLabel={option.label}
                accessibilityRole="radio"
                accessibilityState={{ checked: selected, selected }}
                android_ripple={
                  android
                    ? { color: ripple.neutral, foreground: true }
                    : undefined
                }
                key={String(option.value)}
                onPress={() => {
                  onSelect(option.value);
                  onClose();
                }}
                style={({ pressed }) =>
                  pressed && !android ? styles.pressed : null
                }
                testID={`option-${String(option.value)}`}
              >
                <ListRow
                  right={
                    selected ? (
                      <Icon color="accent" name="check" size={18} />
                    ) : undefined
                  }
                  subtitle={option.subtitle}
                  title={option.label}
                />
              </Pressable>
            );
          })}
        </ListGroup>
      </BottomSheet>
    </Modal>
  );
}

export default OptionSheet;

const styles = StyleSheet.create({
  pressed: { backgroundColor: press.highlight },
});
