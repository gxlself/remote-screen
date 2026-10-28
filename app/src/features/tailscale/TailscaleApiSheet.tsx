import { useRef, useState } from 'react';
import { Modal, Platform, StyleSheet, TextInput, View } from 'react-native';
import type { TextInputProps } from 'react-native';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import { Toast, useToast } from '../../components/Toast';
import { BottomSheet } from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { ListGroup, ListRow } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { common } from '../../i18n/zh/common';
import { usePlatform } from '../../native/platform/PlatformContext';
import { tailscaleApi } from '../../i18n/zh/tailscaleApi';
import { boxHeight } from '../../theme/fontScale';
import { hit, space, tokens } from '../../theme/tokens';
import { fontFor } from '../../theme/typography';
import { errorText, statusText, useTailscaleApi } from './useTailscaleApi';

const copy = tailscaleApi.sheet;
// Where a Tailscale OAuth client (Devices → Core → Read) is created.
const ADMIN_URL = 'https://login.tailscale.com/admin/settings/oauth';

type Busy = 'save' | 'refresh' | 'clear';
type Kind = 'tailscale' | 'headscale';

/** The row in Settings › network access; hidden by the caller on old native packages. */
export function TailscaleApiRow(props: { onPress(): void }) {
  const { status, isLoading, isError } = useTailscaleApi();
  // S12: loading and a failed read are not "未设置".
  const subtitle = isLoading
    ? tailscaleApi.row.loading
    : isError
    ? tailscaleApi.row.loadFailed
    : statusText(status);
  return (
    <ListRow
      chevron
      onPress={props.onPress}
      subtitle={subtitle}
      testID="tailscale-api-row"
      title={tailscaleApi.row.title}
    />
  );
}

type SheetInputProps = Omit<TextInputProps, 'onBlur' | 'onFocus' | 'style'> & {
  invalid: boolean;
};

/** A sheet text field: red border when `invalid`, thicker while focused. */
function SheetInput({ invalid, ...input }: SheetInputProps) {
  const [focused, setFocused] = useState(false);
  const android = Platform.OS === 'android';
  return (
    <TextInput
      {...(android
        ? {
            underlineColorAndroid: 'transparent',
            cursorColor: tokens.accent,
            selectionColor: tokens.accentSoft,
          }
        : {})}
      autoCapitalize="none"
      autoComplete="off"
      autoCorrect={false}
      importantForAutofill="no"
      placeholderTextColor={tokens.text3}
      spellCheck={false}
      {...input}
      onBlur={() => setFocused(false)}
      onFocus={() => setFocused(true)}
      style={[
        styles.input,
        boxHeight(android ? Math.max(46, hit.min) : 46),
        android ? styles.inputAndroid : null,
        focused ? styles.inputFocused : styles.inputIdle,
        {
          borderColor: invalid
            ? tokens.bad
            : focused
            ? tokens.accent
            : tokens.inputLine,
        },
      ]}
    />
  );
}

/**
 * Pick Tailscale or Headscale, then paste, check and save the token. It lives
 * only in this sheet's state until setToken hands it to the native keychain;
 * closing the sheet drops it.
 */
export function TailscaleApiSheet(props: {
  onClose(): void;
  /** Closes the sheet and toasts the message. */
  onDone(message: string): void;
}): React.JSX.Element {
  const { onClose, onDone } = props;
  const platform = usePlatform();
  const { status, save, refresh, clear } = useTailscaleApi();
  const { toast, show, hide } = useToast();
  const [token, setToken] = useState('');
  const [chosenKind, setKind] = useState<Kind | null>(null);
  const [typedUrl, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const running = useRef(false);
  const configured = status?.configured === true;
  const savedUrl = configured ? status.controlUrl ?? null : null;
  const kind = chosenKind ?? (savedUrl === null ? 'tailscale' : 'headscale');
  const headscale = kind === 'headscale';
  const android = Platform.OS === 'android';
  const trimmed = token.trim();
  const trimmedUrl = (typedUrl ?? savedUrl ?? '').trim();
  const ready = trimmed !== '' && (!headscale || trimmedUrl !== '');
  const urlError = error === copy.errors.controlUrl;

  const run = async (
    busyKind: Busy,
    headscaleErrors: boolean,
    action: () => Promise<string | null>,
  ) => {
    if (running.current) {
      return;
    }
    running.current = true;
    setBusy(busyKind);
    setError(null);
    let done: string | null = null;
    try {
      done = await action();
    } catch (failure) {
      setError(errorText(failure, headscaleErrors));
    } finally {
      running.current = false;
      setBusy(null);
    }
    if (done !== null) {
      setToken('');
      onDone(done);
    }
  };

  const close = () => {
    setToken('');
    onClose();
  };

  return (
    <Modal animationType="none" onRequestClose={close} transparent visible>
      <BottomSheet
        footer={
          <View style={styles.actions}>
            <Button
              disabled={!ready || (busy !== null && busy !== 'save')}
              fullWidth
              loading={busy === 'save'}
              onPress={() => {
                if (ready) {
                  const controlUrl = headscale ? trimmedUrl : null;
                  run('save', headscale, async () =>
                    copy.found((await save(trimmed, controlUrl)).length),
                  ).catch(() => undefined);
                }
              }}
              testID="tailscale-api-save"
              title={copy.save}
              variant="primaryLarge"
            />
            {configured ? (
              <>
                <Button
                  disabled={busy !== null && busy !== 'refresh'}
                  fullWidth
                  loading={busy === 'refresh'}
                  onPress={() => {
                    run('refresh', savedUrl !== null, async () => {
                      // S26: the sheet stays; say what the refresh found.
                      show(copy.refreshed((await refresh()).length), {
                        tone: 'good',
                      });
                      return null;
                    }).catch(() => undefined);
                  }}
                  testID="tailscale-api-refresh"
                  title={copy.refresh}
                  variant="secondary"
                />
                <Button
                  disabled={busy !== null}
                  fullWidth
                  onPress={() => setConfirmRemove(true)}
                  testID="tailscale-api-remove"
                  title={copy.remove}
                  variant="pillDanger"
                />
              </>
            ) : null}
          </View>
        }
        onClose={close}
        scroll
        testID="tailscale-api-sheet"
        title={copy.title}
      >
        <View style={styles.content}>
          {configured ? (
            <ListGroup>
              <ListRow
                testID="tailscale-api-status"
                title={copy.status}
                value={statusText(status)}
              />
            </ListGroup>
          ) : null}
          <Segmented
            accessibilityLabel={copy.kind.a11y}
            onChange={next => {
              setKind(next);
              setError(null);
            }}
            options={[
              { value: 'tailscale', label: copy.kind.tailscale },
              { value: 'headscale', label: copy.kind.headscale },
            ]}
            testID="tailscale-api-kind"
            value={kind}
          />
          {headscale ? (
            <>
              <AppText color="text2" variant="body">
                {copy.headscale.lead}
              </AppText>
              <AppText color="text2" variant="footnote">
                {copy.headscale.apiKey}
              </AppText>
              <AppText color="text3" variant="footnote">
                {android
                  ? copy.headscale.storageAndroid
                  : copy.headscale.storage}
              </AppText>
              <SheetInput
                accessibilityLabel={copy.controlUrlLabel}
                editable={busy === null}
                invalid={urlError}
                keyboardType="url"
                onChangeText={value => {
                  setUrl(value);
                  setError(null);
                }}
                placeholder={copy.controlUrlPlaceholder}
                testID="tailscale-api-control-url"
                textContentType="URL"
                value={typedUrl ?? savedUrl ?? ''}
              />
            </>
          ) : (
            <>
              <AppText color="text2" variant="body">
                {copy.lead}
              </AppText>
              <AppText color="text2" variant="footnote">
                {copy.oauth}
              </AppText>
              <AppText color="text3" variant="footnote">
                {copy.apiKey}
              </AppText>
              <AppText color="text3" variant="footnote">
                {android ? copy.storageAndroid : copy.storage}
              </AppText>
              <Button
                align="start"
                icon="external"
                onPress={() => {
                  platform.openUrl(ADMIN_URL).catch(() => undefined);
                }}
                testID="tailscale-api-admin"
                title={copy.openAdmin}
                variant="text"
              />
            </>
          )}
          <SheetInput
            accessibilityLabel={
              headscale ? copy.headscale.tokenLabel : copy.tokenLabel
            }
            editable={busy === null}
            invalid={error !== null && !urlError}
            onChangeText={value => {
              setToken(value);
              setError(null);
            }}
            placeholder={
              headscale ? copy.headscale.placeholder : copy.placeholder
            }
            secureTextEntry
            testID="tailscale-api-token"
            textContentType="none"
            value={token}
          />
          {error === null ? null : (
            <Banner testID="tailscale-api-error" title={error} tone="bad" />
          )}
        </View>
      </BottomSheet>
      <Toast onHide={hide} toast={toast} />
      <Dialog
        actions={[
          {
            label: copy.removeDialog.confirm,
            onPress: () => {
              setConfirmRemove(false);
              run('clear', false, async () => {
                await clear();
                return copy.removed;
              }).catch(() => undefined);
            },
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setConfirmRemove(false),
            variant: 'secondary',
          },
        ]}
        message={copy.removeDialog.message}
        onDismiss={() => setConfirmRemove(false)}
        testID="tailscale-api-remove-dialog"
        title={copy.removeDialog.title}
        visible={confirmRemove}
      />
    </Modal>
  );
}

const styles = StyleSheet.create({
  content: { gap: space.md },
  actions: { gap: space.sm },
  input: {
    borderRadius: 12,
    paddingHorizontal: 12,
    backgroundColor: tokens.card,
    color: tokens.text,
    textAlign: 'left',
    fontFamily: fontFor(400, true),
    fontSize: 14,
  },
  inputAndroid: {
    paddingVertical: 0,
    textAlignVertical: 'center',
  },
  inputIdle: {
    borderWidth: 1,
  },
  inputFocused: {
    borderWidth: 2,
  },
});
