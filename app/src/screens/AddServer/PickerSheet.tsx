import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { BottomSheet } from '../../components/BottomSheet';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { TextField } from '../../components/TextField';
import { Button } from '../../components/Button';
import { addServer } from '../../i18n/zh/addServer';
import { space } from '../../theme/tokens';

export type JumpOption = {
  id: string;
  name: string;
  host: string;
  port: number;
};

export function PickerSheet({
  visible,
  title,
  children,
  onClose,
  footer,
  testID,
}: {
  visible: boolean;
  title: string;
  children: ReactNode;
  onClose: () => void;
  footer?: ReactNode;
  testID?: string;
}) {
  if (!visible) {
    return null;
  }
  return (
    <BottomSheet
      onClose={onClose}
      scroll
      testID={testID}
      title={title}
      footer={footer}
    >
      {children}
    </BottomSheet>
  );
}

export function JumpPicker({
  visible,
  servers,
  onSelect,
  onOther,
  onClose,
}: {
  visible: boolean;
  servers: JumpOption[];
  onSelect: (server: JumpOption) => void;
  /** Returns why the address cannot be used, or null once it is taken. */
  onOther: (address: string) => string | null;
  onClose: () => void;
}) {
  const [other, setOther] = useState('');
  const [otherError, setOtherError] = useState<string | null>(null);

  useEffect(() => {
    if (!visible) {
      setOther('');
      setOtherError(null);
    }
  }, [visible]);

  const takeOther = () => {
    const problem = onOther(other);
    setOtherError(problem);
    if (problem === null) {
      onClose();
    }
  };

  return (
    <PickerSheet
      onClose={onClose}
      testID="jump-picker"
      title={addServer.jump.sheetTitle}
      visible={visible}
      footer={
        <View style={styles.other}>
          <TextField
            autoCapitalize="none"
            autoCorrect={false}
            error={otherError ?? undefined}
            keyboardType="url"
            label={addServer.jump.otherLabel}
            onChangeText={value => {
              setOther(value);
              setOtherError(null);
            }}
            onSubmitEditing={takeOther}
            placeholder={addServer.jump.otherPlaceholder}
            returnKeyType="done"
            testID="jump-other-input"
            value={other}
          />
          <Button
            fullWidth
            onPress={takeOther}
            title={addServer.jump.useOther}
            variant="secondary"
          />
        </View>
      }
    >
      {servers.length === 0 ? (
        <AppText color="text2" variant="footnote" testID="jump-no-servers">
          {addServer.jump.noServers}
        </AppText>
      ) : (
        <ListGroup>
          {servers.map(server => (
            <ListRow
              key={server.id}
              onPress={() => {
                onSelect(server);
                onClose();
              }}
              subtitle={`${server.host}:${server.port}`}
              title={server.name}
            />
          ))}
        </ListGroup>
      )}
    </PickerSheet>
  );
}

export function NetworkPicker({
  visible,
  networks,
  current,
  onSelect,
  onClose,
}: {
  visible: boolean;
  networks: Array<{ id: string; name: string }>;
  current: string | null;
  onSelect: (id: string | null) => void;
  onClose: () => void;
}) {
  return (
    <PickerSheet
      onClose={onClose}
      testID="network-picker"
      title={addServer.network.sheetTitle}
      visible={visible}
    >
      <ListGroup>
        <ListRow
          accessibilityLabel={
            current === null
              ? addServer.network.selectedA11y(addServer.network.none)
              : addServer.network.none
          }
          onPress={() => {
            onSelect(null);
            onClose();
          }}
          right={current === null ? <SelectedMark /> : undefined}
          testID="network-option-none"
          title={addServer.network.none}
        />
        {networks.map(network => (
          <ListRow
            accessibilityLabel={
              current === network.id
                ? addServer.network.selectedA11y(network.name)
                : network.name
            }
            key={network.id}
            onPress={() => {
              onSelect(network.id);
              onClose();
            }}
            right={current === network.id ? <SelectedMark /> : undefined}
            testID={`network-option-${network.id}`}
            title={network.name}
          />
        ))}
      </ListGroup>
    </PickerSheet>
  );
}

/** iOS marks the current choice with a check, not a disclosure chevron. */
function SelectedMark() {
  return (
    <View testID="picker-selected">
      <Icon color="accent" name="check" size={18} />
    </View>
  );
}

const styles = StyleSheet.create({
  other: {
    gap: space.md,
  },
});

export default PickerSheet;
