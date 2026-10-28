import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  Platform,
  PixelRatio,
  StyleSheet,
  View,
} from 'react-native';
import type { NavigationAction } from '@react-navigation/native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { EmptyState } from '../../components/EmptyState';
import { FormScroll } from '../../components/FormScroll';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { Switch } from '../../components/Switch';
import { TextField } from '../../components/TextField';
import type { TextFieldHandle } from '../../components/TextField';
import { useSshKeys } from '../../features/data/queries';
import type { FieldKey, FormErrorCode } from '../../features/ssh/serverForm';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import {
  parseHostPort,
  shouldSuggestJump,
} from '../../features/ssh/serverForm';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { SshAuth } from '../../native/rscore/types';
import { addServer } from '../../i18n/zh/addServer';
import { common } from '../../i18n/zh/common';
import { space, tokens } from '../../theme/tokens';
import { AuthSection } from './AuthSection';
import { JumpPicker, NetworkPicker } from './PickerSheet';
import { TestResult } from './TestResult';
import { useAddServer } from './useAddServer';

function errorText(code: FormErrorCode | undefined): string | undefined {
  return code === undefined ? undefined : addServer.errors[code];
}

export default function AddServerScreen(props: RootScreenProps<'AddServer'>) {
  const { navigation } = props;
  const insets = useSafeAreaInsets();
  const platform = usePlatform();
  const keysQuery = useSshKeys();
  const controller = useAddServer(props);
  const tailnetEnabled = useTailnetEnabled();
  const {
    form,
    errors,
    networks,
    networkId,
    jumpServers,
    jumpAddress,
    tailscaleUnavailable,
    testState,
    savedError,
    deleteError,
    saving,
    deleting,
    firstError,
    dirty,
    leaving,
    notFound,
    readOnly,
    loading,
    setField,
    setAuth,
    setJumpOn,
    selectNetwork,
    testConnection,
    save,
    remove,
  } = controller;
  const [jumpPickerOpen, setJumpPickerOpen] = useState(false);
  const [networkPickerOpen, setNetworkPickerOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [discardOpen, setDiscardOpen] = useState(false);
  const pendingLeave = useRef<NavigationAction | null>(null);
  const nameRef = useRef<TextFieldHandle>(null);
  const userRef = useRef<TextFieldHandle>(null);
  const hostRef = useRef<TextFieldHandle>(null);
  const portRef = useRef<TextFieldHandle>(null);
  const sheetOpen = jumpPickerOpen || networkPickerOpen;
  const largeText = PixelRatio.getFontScale() > 2;

  const networkName = useMemo(
    () => networks.find(network => network.id === networkId)?.name ?? null,
    [networkId, networks],
  );
  const idleHint = tailscaleUnavailable
    ? addServer.test.idleTailscale
    : shouldSuggestJump(form, networks)
    ? addServer.test.idleJump(form.host.trim())
    : addServer.test.idleHint;
  const editing = controller.serverId !== undefined;
  const title = editing ? addServer.title.edit : addServer.title.add;
  const keys = keysQuery.data ?? [];
  const jumpLabel =
    form.jump.trim() === ''
      ? addServer.jump.choose
      : addServer.jump.current(form.jump.trim());
  const networkLabel = addServer.network.a11y(
    networkName ?? addServer.network.none,
  );

  const fieldError = (key: FieldKey) => errorText(errors[key]);
  const fixAbove =
    firstError === null
      ? null
      : addServer.fixAbove(addServer.fieldNames[firstError]);

  // Closing with unsaved changes asks first; saving or deleting leaves freely.
  useEffect(
    () =>
      navigation.addListener('beforeRemove', event => {
        if (!dirty || readOnly || leaving.current) {
          return;
        }
        event.preventDefault();
        pendingLeave.current = event.data.action;
        setDiscardOpen(true);
      }),
    [dirty, leaving, navigation, readOnly],
  );

  const discard = () => {
    const action = pendingLeave.current;
    pendingLeave.current = null;
    setDiscardOpen(false);
    leaving.current = true;
    if (action === null) {
      navigation.goBack();
    } else {
      navigation.dispatch(action);
    }
  };

  const submit = async () => {
    const invalid = await save();
    if (invalid === null) {
      return;
    }
    const field = {
      name: nameRef,
      user: userRef,
      host: hostRef,
      port: portRef,
      auth: null,
      jump: null,
    }[invalid];
    // A text field scrolls itself into view when it takes focus; for the rest
    // the line next to the save button names what to fix.
    field?.current?.focus();
    if (Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(
        addServer.fixAbove(addServer.fieldNames[invalid]),
      );
    }
  };

  const backButton = (
    <IconButton
      accessibilityLabel={common.a11y.back}
      icon="chevronLeft"
      onPress={() => navigation.goBack()}
      size={44}
      variant="raised"
    />
  );

  if (notFound) {
    return (
      <View style={[styles.screen, { paddingTop: insets.top + 8 }]}>
        <EmptyState
          action={{
            label: common.back,
            onPress: () => navigation.goBack(),
          }}
          icon="computerOff"
          title={addServer.notFound.title}
        />
      </View>
    );
  }

  if (loading) {
    return (
      <View
        style={[styles.screen, { paddingTop: insets.top + 8 }]}
        testID="add-server-loading"
      >
        <View style={styles.header}>{backButton}</View>
        <View style={styles.loading}>
          <ActivityIndicator color={tokens.text2} />
          <AppText center color="text2">
            {common.loading}
          </AppText>
        </View>
      </View>
    );
  }

  // Tailscale SSH is only offered with Tailscale on; a server already saved
  // with it keeps the option so its form still shows what it uses.
  const authOptions: Array<{ value: SshAuth; label: string }> = [
    ...(tailnetEnabled || form.auth === 'tailscale'
      ? [{ value: 'tailscale' as const, label: addServer.auth.options.tailscale }]
      : []),
    { value: 'deviceKey', label: addServer.auth.options.deviceKey },
    { value: 'password', label: addServer.auth.options.password },
  ];

  const hiddenBehindSheet =
    Platform.OS === 'android'
      ? sheetOpen
        ? 'no-hide-descendants'
        : 'auto'
      : undefined;

  return (
    <FormScroll
      contentContainerStyle={[
        styles.content,
        { paddingBottom: insets.bottom + 24 },
      ]}
      footer={
        <>
          <JumpPicker
            onClose={() => setJumpPickerOpen(false)}
            onOther={value => {
              const address = value.trim();
              if (address === '') {
                return addServer.jump.otherEmpty;
              }
              if (parseHostPort(address, 22) === null) {
                return addServer.jump.otherBad;
              }
              setField('jump', address);
              return null;
            }}
            onSelect={server => setField('jump', server.name)}
            servers={jumpServers}
            visible={jumpPickerOpen}
          />
          <NetworkPicker
            current={networkId}
            networks={networks}
            onClose={() => setNetworkPickerOpen(false)}
            onSelect={selectNetwork}
            visible={networkPickerOpen}
          />
          <Dialog
            actions={[
              {
                label: common.delete,
                loading: deleting,
                onPress: () => {
                  remove().then(removed => {
                    if (!removed) {
                      setDeleteOpen(false);
                    }
                  });
                },
                testID: 'delete-server-confirm',
                variant: 'pillDanger',
              },
              {
                label: common.cancel,
                onPress: () => setDeleteOpen(false),
                variant: 'secondary',
              },
            ]}
            message={addServer.delete.message}
            onDismiss={() => setDeleteOpen(false)}
            testID="delete-server-dialog"
            title={addServer.delete.title(controller.editingName ?? form.name)}
            visible={deleteOpen}
          />
          <Dialog
            actions={[
              {
                label: addServer.discard.confirm,
                onPress: discard,
                testID: 'discard-confirm',
                variant: 'pillDanger',
              },
              {
                label: addServer.discard.keep,
                onPress: () => {
                  pendingLeave.current = null;
                  setDiscardOpen(false);
                },
                variant: 'secondary',
              },
            ]}
            message={addServer.discard.message}
            onDismiss={() => {
              pendingLeave.current = null;
              setDiscardOpen(false);
            }}
            testID="discard-dialog"
            title={addServer.discard.title}
            visible={discardOpen}
          />
        </>
      }
      header={
        <View
          importantForAccessibility={hiddenBehindSheet}
          style={[styles.header, { paddingTop: insets.top + 8 }]}
        >
          {backButton}
        </View>
      }
      importantForAccessibility={hiddenBehindSheet}
      style={styles.screen}
      testID="add-server-form"
    >
      <AppText accessibilityRole="header" variant="title">
        {title}
      </AppText>

      {readOnly ? (
        <AppText color="relay" variant="footnote" testID="read-only-note">
          {addServer.readOnly}
        </AppText>
      ) : null}

      <View style={styles.form}>
        <TextField
          blurOnSubmit={false}
          editable={!readOnly}
          label={addServer.fields.name.label}
          onChangeText={value => setField('name', value)}
          onSubmitEditing={() => userRef.current?.focus()}
          placeholder={addServer.fields.name.placeholder}
          error={fieldError('name')}
          ref={nameRef}
          returnKeyType="next"
          value={form.name}
        />
        <TextField
          autoCapitalize="none"
          autoCorrect={false}
          autoFocus={controller.prefilled}
          blurOnSubmit={false}
          editable={!readOnly}
          label={addServer.fields.user.label}
          mono
          onChangeText={value => setField('user', value)}
          onSubmitEditing={() => hostRef.current?.focus()}
          error={fieldError('user')}
          ref={userRef}
          returnKeyType="next"
          textContentType="username"
          value={form.user}
        />
        <View style={[styles.hostRow, largeText ? styles.hostRowLarge : null]}>
          <View
            style={[styles.hostField, largeText ? styles.hostFieldLarge : null]}
          >
            <TextField
              autoCapitalize="none"
              autoCorrect={false}
              blurOnSubmit={false}
              editable={!readOnly}
              error={fieldError('host')}
              hint={addServer.fields.host.hint}
              keyboardType="url"
              label={addServer.fields.host.label}
              mono
              onChangeText={value => setField('host', value)}
              onSubmitEditing={() => portRef.current?.focus()}
              ref={hostRef}
              returnKeyType="next"
              value={form.host}
            />
          </View>
          <View
            style={[styles.portField, largeText ? styles.portFieldLarge : null]}
          >
            <TextField
              editable={!readOnly}
              error={fieldError('port')}
              keyboardType="number-pad"
              label={addServer.fields.port.label}
              maxLength={5}
              mono
              onChangeText={value => setField('port', value)}
              ref={portRef}
              returnKeyType="done"
              value={form.port}
            />
          </View>
        </View>
      </View>

      <View style={styles.section}>
        <SectionHeader title={addServer.auth.title} />
        <View pointerEvents={readOnly ? 'none' : 'auto'}>
          <Segmented
            accessibilityLabel={addServer.auth.title}
            onChange={readOnly ? () => {} : setAuth}
            options={authOptions}
            testID="add-server-auth"
            value={form.auth}
            variant="page"
          />
        </View>
        <AuthSection
          auth={form.auth}
          keys={keys}
          onCopy={text => platform.copyText(text)}
          onOpenKeys={() => navigation.navigate('SshKeys')}
          error={fieldError('auth')}
          readOnly={readOnly}
          tailscaleUnavailable={tailscaleUnavailable}
        />
      </View>

      <View style={styles.section}>
        <ListGroup>
          <ListRow
            right={
              <Switch
                accessibilityLabel={addServer.jump.title}
                disabled={readOnly}
                onValueChange={setJumpOn}
                value={form.jumpOn}
              />
            }
            subtitle={addServer.jump.subtitle}
            title={addServer.jump.title}
          />
          {form.jumpOn ? (
            <ListRow
              accessibilityLabel={jumpLabel}
              disabled={readOnly}
              onPress={() => setJumpPickerOpen(true)}
              right={
                <View style={styles.jumpMeta}>
                  {jumpAddress === null ? null : (
                    <AppText color="text2" mono variant="monoSmall">
                      {jumpAddress}
                    </AppText>
                  )}
                  <AppText color="text3" variant="caption">
                    {addServer.jump.sameKey}
                  </AppText>
                </View>
              }
              title={
                form.jump.trim() === '' ? addServer.jump.choose : form.jump
              }
            />
          ) : null}
        </ListGroup>
        {errors.jump === undefined ? null : (
          <AppText color="bad" variant="caption" testID="jump-error">
            {fieldError('jump')}
          </AppText>
        )}
      </View>

      {networks.length === 0 ? null : (
        <ListGroup>
          <ListRow
            accessibilityLabel={networkLabel}
            disabled={readOnly}
            onPress={() => setNetworkPickerOpen(true)}
            right={
              <AppText color="text2" variant="footnote">
                {networkName ?? addServer.network.none}
              </AppText>
            }
            subtitle={addServer.network.subtitle}
            title={addServer.network.title}
          />
        </ListGroup>
      )}

      <TestResult idleHint={idleHint} state={testState} />

      <View style={styles.actions}>
        <Button
          fullWidth
          loading={testState.kind === 'running'}
          onPress={() => {
            testConnection();
          }}
          title={addServer.actions.test}
          variant="secondary"
        />
        {readOnly ? null : (
          <Button
            disabled={saving}
            fullWidth
            loading={saving}
            onPress={() => {
              submit();
            }}
            testID="add-server-save"
            title={
              controller.connectAfterSave
                ? addServer.actions.saveAndConnect
                : common.save
            }
            variant="primary"
          />
        )}
        {fixAbove === null || readOnly ? null : (
          <AppText color="bad" variant="footnote" testID="add-server-fix-above">
            {fixAbove}
          </AppText>
        )}
        {savedError === null ? null : (
          <AppText color="bad" variant="footnote" testID="save-error">
            {savedError}
          </AppText>
        )}
      </View>

      {editing && !readOnly ? (
        <View style={styles.section}>
          <ListGroup>
            <ListRow
              disabled={deleting}
              onPress={() => setDeleteOpen(true)}
              title={addServer.delete.row}
              tone="danger"
            />
          </ListGroup>
          {deleteError === null ? null : (
            <AppText color="bad" variant="footnote" testID="delete-error">
              {deleteError}
            </AppText>
          )}
        </View>
      ) : null}
    </FormScroll>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  header: {
    paddingHorizontal: space.lg,
    paddingBottom: space.sm,
  },
  loading: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    gap: space.md,
  },
  content: {
    paddingHorizontal: space.gutter,
    paddingTop: space.sm,
    gap: space.section,
  },
  form: {
    gap: space.lg,
  },
  hostRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: space.md,
  },
  hostRowLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  hostField: {
    flex: 1,
  },
  hostFieldLarge: {
    flex: 0,
  },
  portField: {
    width: 96,
  },
  portFieldLarge: {
    width: undefined,
  },
  section: {
    gap: space.md,
  },
  jumpMeta: {
    alignItems: 'flex-end',
    gap: 2,
  },
  actions: {
    gap: 10,
  },
});
