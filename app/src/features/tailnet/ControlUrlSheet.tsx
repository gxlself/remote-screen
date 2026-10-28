import { useState } from 'react';
import { Modal, StyleSheet, View } from 'react-native';
import BottomSheet from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import AppText from '../../components/AppText';
import { Dialog } from '../../components/Dialog';
import { TextField } from '../../components/TextField';
import { common } from '../../i18n/zh/common';
import { tailnet } from '../../i18n/zh/tailnet';
import { space } from '../../theme/tokens';
import { useSetting } from '../settings/store';
import { normalizeControlUrl } from './controlUrl';
import { useEmbeddedSwitch } from './useEmbeddedSwitch';

// Android back while saving: keep the sheet up.
function ignoreRequestClose() {}

/**
 * Its own transparent Modal, so wherever it is opened from (a settings
 * scroll view, a home banner, an error board) it covers the whole window.
 */
export function ControlUrlSheet(props: {
  onClose(): void;
  onDone(result: 'switched' | 'unavailable' | 'saved'): void;
  /** true：只保存地址，不切模式（设置页「Headscale 地址」一行用） */
  saveOnly?: boolean;
}): React.JSX.Element {
  const { onClose, onDone, saveOnly = false } = props;
  const [storedUrl, setStoredUrl] = useSetting('tailnet.controlUrl');
  const { saveControlUrlAndSwitch } = useEmbeddedSwitch();
  const [text, setText] = useState(storedUrl ?? '');
  const [error, setError] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  // S08: replacing or clearing a saved address waits for a yes.
  const [confirm, setConfirm] = useState<'change' | 'clear' | null>(null);

  const apply = async () => {
    setLoading(true);
    let done: 'switched' | 'unavailable' | 'saved';
    try {
      if (saveOnly) {
        setStoredUrl(normalizeControlUrl(text));
        done = 'saved';
      } else {
        const result = await saveControlUrlAndSwitch(text);
        if (result === 'invalid') {
          setError(tailnet.controlUrl.invalid);
          return;
        }
        done = result;
      }
      setError(undefined);
    } finally {
      setLoading(false);
    }
    onDone(done);
  };

  const submit = async () => {
    if (loading) {
      return;
    }
    const normalized = normalizeControlUrl(text);
    const blank = text.trim() === '';
    if (normalized === null && (!saveOnly || !blank)) {
      setError(tailnet.controlUrl.invalid);
      return;
    }
    if (storedUrl !== null && normalized !== storedUrl) {
      setConfirm(normalized === null ? 'clear' : 'change');
      return;
    }
    await apply();
  };

  const dialog =
    confirm === 'clear'
      ? tailnet.controlUrl.clearDialog
      : tailnet.controlUrl.changeDialog;

  return (
    <Modal
      animationType="none"
      onRequestClose={loading ? ignoreRequestClose : onClose}
      transparent
      visible
    >
      <BottomSheet
        dismissible={!loading}
        footer={
          <Button
            fullWidth
            loading={loading}
            onPress={() => {
              submit().catch(() => undefined);
            }}
            testID="tailnet-control-url-submit"
            title={saveOnly ? common.save : common.continue}
            variant="primaryLarge"
          />
        }
        onClose={onClose}
        testID="tailnet-control-url-sheet"
        title={tailnet.controlUrl.title}
      >
        <View style={styles.content}>
          <TextField
            accessibilityLabel={tailnet.controlUrl.label}
            autoCapitalize="none"
            autoCorrect={false}
            error={error}
            keyboardType="url"
            label={tailnet.controlUrl.label}
            onChangeText={value => {
              setText(value);
              if (error !== undefined) {
                setError(undefined);
              }
            }}
            onSubmitEditing={() => {
              submit().catch(() => undefined);
            }}
            placeholder={tailnet.controlUrl.placeholder}
            returnKeyType="done"
            testID="tailnet-control-url-input"
            textContentType="URL"
            value={text}
          />
          <AppText color="text3" variant="footnote">
            {saveOnly
              ? tailnet.controlUrl.saveOnlyNote
              : tailnet.controlUrl.note}
          </AppText>
        </View>
      </BottomSheet>
      <Dialog
        actions={[
          {
            label: dialog.confirm,
            onPress: () => {
              setConfirm(null);
              apply().catch(() => undefined);
            },
            testID: 'tailnet-control-url-confirm',
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setConfirm(null),
            variant: 'secondary',
          },
        ]}
        message={dialog.message}
        onDismiss={() => setConfirm(null)}
        testID="tailnet-control-url-dialog"
        title={dialog.title}
        visible={confirm !== null}
      />
    </Modal>
  );
}

export default ControlUrlSheet;

const styles = StyleSheet.create({
  content: { gap: space.md },
});
