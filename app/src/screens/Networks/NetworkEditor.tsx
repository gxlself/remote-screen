import { useState } from 'react';
import {
  AccessibilityInfo,
  Modal,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import AppText from '../../components/AppText';
import { BottomSheet } from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { ListRow } from '../../components/List';
import { TextField } from '../../components/TextField';
import { showToast } from '../../components/ToastHost';
import {
  formToNetwork,
  networkToForm,
  newNetworkId,
  parseCidrList,
  validateNetworkForm,
} from '../../features/network/networks';
import type { NetworkForm } from '../../features/network/networks';
import { useNetworks } from '../../features/network/useNetworks';
import type { NetworkEntry } from '../../features/settings/schema';
import { common } from '../../i18n/zh/common';
import { networks as copy } from '../../i18n/zh/networks';
import { space } from '../../theme/tokens';

type Props = {
  visible: boolean;
  entry: NetworkEntry | null;
  initialCidrs: string;
  onClose: () => void;
};

type FieldErrors = Partial<
  Record<
    'name' | 'cidrs' | 'vpnPools',
    'required' | 'badCidr' | 'duplicateName'
  >
>;

function initialForm(
  entry: NetworkEntry | null,
  initialCidrs: string,
): NetworkForm {
  if (entry !== null) {
    return networkToForm(entry);
  }
  return { name: '', cidrs: initialCidrs, vpnPools: '' };
}

export default function NetworkEditor({
  visible,
  entry,
  initialCidrs,
  onClose,
}: Props) {
  if (!visible) {
    return null;
  }
  return (
    <NetworkEditorSheet
      entry={entry}
      initialCidrs={initialCidrs}
      onClose={onClose}
    />
  );
}

function NetworkEditorSheet({
  entry,
  initialCidrs,
  onClose,
}: Omit<Props, 'visible'>) {
  const { items, save, remove } = useNetworks();
  const [form, setForm] = useState<NetworkForm>(() =>
    initialForm(entry, initialCidrs),
  );
  const [errors, setErrors] = useState<FieldErrors>({});
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [deleteVisible, setDeleteVisible] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const isEditing = entry !== null;
  const title = isEditing ? copy.editor.editTitle : copy.editor.addTitle;

  const close = () => {
    setErrors({});
    setSaveError(null);
    onClose();
  };

  const update = (field: keyof NetworkForm, value: string) => {
    setForm(current => ({ ...current, [field]: value }));
    setErrors(current => {
      if (current[field] === undefined) {
        return current;
      }
      const next = { ...current };
      delete next[field];
      return next;
    });
    setSaveError(null);
  };

  const errorText = (
    field: keyof NetworkForm,
    value: string,
  ): string | undefined => {
    const error = errors[field];
    if (error === undefined) {
      return undefined;
    }
    if (error === 'required') {
      return copy.editor.error.required;
    }
    if (error === 'duplicateName') {
      return copy.editor.error.duplicateName;
    }
    return copy.editor.error.badCidr(parseCidrList(value).bad);
  };

  const validationText = (
    error: NonNullable<FieldErrors[keyof NetworkForm]>,
    value: string,
  ): string => {
    if (error === 'required') {
      return copy.editor.error.required;
    }
    if (error === 'duplicateName') {
      return copy.editor.error.duplicateName;
    }
    return copy.editor.error.badCidr(parseCidrList(value).bad);
  };

  const showSaveError = (message: string) => {
    setSaveError(message);
    if (Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(message);
    }
  };

  const onSave = async () => {
    const nextErrors = validateNetworkForm(form, {
      existingNames: items.map(item => item.name),
      editingName: entry?.name,
    });
    setErrors(nextErrors);
    if (Object.keys(nextErrors).length > 0) {
      const fieldOrder: (keyof NetworkForm)[] = ['name', 'cidrs', 'vpnPools'];
      const field = fieldOrder.find(candidate => nextErrors[candidate]);
      if (field !== undefined) {
        const error = nextErrors[field];
        if (error !== undefined) {
          const message = validationText(error, form[field]);
          AccessibilityInfo.announceForAccessibility(
            [copy.editor[field].label, message].join(' '),
          );
        }
      }
      return;
    }

    const taken = items.map(item => item.id).filter(id => id !== entry?.id);
    const id = entry?.id ?? newNetworkId(form.name, taken);
    setSaving(true);
    setSaveError(null);
    try {
      await save(formToNetwork(form, id));
      setSaving(false);
      close();
      showToast({ message: copy.editor.saved, tone: 'good' });
    } catch (error) {
      setSaving(false);
      showSaveError(
        copy.editor.error.save(
          error instanceof Error ? error.message : String(error),
        ),
      );
    }
  };

  const onDelete = async () => {
    if (entry === null) {
      return;
    }
    setDeleting(true);
    setSaveError(null);
    try {
      await remove(entry.id);
      setDeleting(false);
      setDeleteVisible(false);
      close();
      showToast({ message: copy.editor.deleted, tone: 'good' });
    } catch (error) {
      setDeleting(false);
      setDeleteVisible(false);
      showSaveError(
        copy.editor.error.delete(
          error instanceof Error ? error.message : String(error),
        ),
      );
    }
  };

  return (
    <Modal animationType="none" onRequestClose={close} transparent visible>
      <BottomSheet onClose={close} scroll testID="network-editor" title={title}>
        <View style={styles.form}>
          <TextField
            error={errorText('name', form.name)}
            label={copy.editor.name.label}
            onChangeText={value => update('name', value)}
            placeholder={copy.editor.name.placeholder}
            testID="network-name"
            value={form.name}
          />
          <TextField
            error={errorText('cidrs', form.cidrs)}
            keyboardType="numbers-and-punctuation"
            label={copy.editor.cidrs.label}
            mono
            onChangeText={value => update('cidrs', value)}
            placeholder={copy.editor.cidrs.placeholder}
            testID="network-cidrs"
            value={form.cidrs}
          />
          <TextField
            error={errorText('vpnPools', form.vpnPools)}
            keyboardType="numbers-and-punctuation"
            label={copy.editor.vpnPools.label}
            mono
            onChangeText={value => update('vpnPools', value)}
            placeholder={copy.editor.vpnPools.placeholder}
            testID="network-vpn-pools"
            value={form.vpnPools}
          />
          {saveError === null ? null : (
            <AppText
              accessibilityLiveRegion="polite"
              color="bad"
              variant="caption"
            >
              {saveError}
            </AppText>
          )}
          <Button
            fullWidth
            loading={saving}
            onPress={onSave}
            testID="network-save"
            title={copy.editor.save}
          />
        </View>
        {isEditing ? (
          <ListRow
            accessibilityLabel={copy.editor.deleteA11y(entry.name)}
            onPress={() => setDeleteVisible(true)}
            testID="network-delete"
            title={copy.editor.delete}
            tone="danger"
          />
        ) : null}
      </BottomSheet>
      <Dialog
        actions={[
          {
            label: copy.editor.deleteDialog.confirm,
            loading: deleting,
            onPress: () => {
              onDelete().catch(() => undefined);
            },
            testID: 'delete-network-confirm',
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setDeleteVisible(false),
            variant: 'secondary',
          },
        ]}
        message={copy.editor.deleteDialog.message}
        onDismiss={deleting ? undefined : () => setDeleteVisible(false)}
        testID="delete-network-dialog"
        title={copy.editor.deleteDialog.title(entry?.name ?? '')}
        visible={deleteVisible}
      />
    </Modal>
  );
}

const styles = StyleSheet.create({
  form: {
    gap: space.lg,
  },
});
