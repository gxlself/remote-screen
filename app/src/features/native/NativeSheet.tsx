import { useEffect, useRef, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import BottomSheet from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { StatusChip } from '../../components/StatusChip';
import { TextField } from '../../components/TextField';
import type { TextFieldHandle } from '../../components/TextField';
import { Toast, useToast } from '../../components/Toast';
import { showToast } from '../../components/ToastHost';
import { common } from '../../i18n/zh/common';
import { deviceSheet } from '../../i18n/zh/deviceSheet';
import { native } from '../../i18n/zh/native';
import { tokens } from '../../theme/tokens';
import {
  entryReach,
  nativeCardModel,
  type NativeEntry,
} from '../directory/cardModel';
import { FallbackGuide } from '../fallback/FallbackGuide';
import { useDirectory } from '../data/queries';
import { RDP_PORT, VNC_PORT } from '../servers/addressProbe';
import { hostKind, parseHostPort } from '../ssh/serverForm';
import { useManualServers } from '../servers/useManualServers';
import { entryProtocol } from './entries';
import { useManualReach } from './reach';
import { useComputerEntries } from './useComputerEntries';
import { useConnect } from './useConnect';

type NativeProtocol = 'vnc' | 'rdp';

const PROTOCOL_OS = { vnc: 'macos', rdp: 'windows' } as const;
const PROTOCOL_PORT = { vnc: VNC_PORT, rdp: RDP_PORT } as const;
const PROTOCOL_OPTIONS: { value: NativeProtocol; label: string }[] = [
  { value: 'vnc', label: native.sheet.protocolName.vnc },
  { value: 'rdp', label: native.sheet.protocolName.rdp },
];

/** V10 C8 device card for a computer without Host (tailnet or manual). */
export default function NativeSheetScreen({
  navigation,
  route,
}: RootScreenProps<'NativeSheet'>) {
  const { entryKey } = route.params;
  const entries = useComputerEntries();
  const directoryQuery = useDirectory();
  const manualServers = useManualServers();
  const { connect, busy, guide, closeGuide } = useConnect();
  const { toast, show, hide } = useToast();
  const [focused, setFocused] = useState(() => navigation.isFocused());
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState('');
  const [hostText, setHostText] = useState('');
  const [portText, setPortText] = useState('');
  const [editError, setEditError] = useState<{
    host?: string;
    port?: string;
  }>({});
  const [savingEdit, setSavingEdit] = useState(false);
  const [savingProtocol, setSavingProtocol] = useState(false);
  const hostRef = useRef<TextFieldHandle>(null);
  const portRef = useRef<TextFieldHandle>(null);
  const [confirmingRemove, setConfirmingRemove] = useState(false);
  const [removing, setRemoving] = useState(false);
  // Keeps the sheet on screen between the delete write and closing.
  const [removed, setRemoved] = useState<NativeEntry | null>(null);

  const found = entries.find(candidate => candidate.key === entryKey);
  const entry: NativeEntry | undefined =
    found === undefined || found.kind === 'host' ? removed ?? undefined : found;
  const manualReach = useManualReach(
    entry === undefined ? [] : [entry],
    focused,
  );

  useEffect(() => {
    const offFocus = navigation.addListener('focus', () => setFocused(true));
    const offBlur = navigation.addListener('blur', () => setFocused(false));
    return () => {
      offFocus();
      offBlur();
    };
  }, [navigation]);

  const close = () => navigation.goBack();

  if (entry === undefined) {
    const loading = directoryQuery.isLoading || manualServers.isLoading;
    return (
      <BottomSheet
        onClose={close}
        title={loading ? deviceSheet.title : native.sheet.notFound}
      >
        <View style={styles.message}>
          {loading ? (
            <AppText color="text3">{common.loading}</AppText>
          ) : (
            <Button
              fullWidth
              onPress={close}
              title={common.close}
              variant="secondary"
            />
          )}
        </View>
      </BottomSheet>
    );
  }

  const reach = entryReach(entry, manualReach);
  const card = nativeCardModel(entry, reach);
  const manual = entry.kind === 'manual' ? entry.manual : null;
  const spokenReach =
    reach === 'unknown' ? native.a11y.reachUnknown : card.reachText;

  const defaultPort = manual?.protocol === 'rdp' ? RDP_PORT : VNC_PORT;

  const startRename = () => {
    setName(manual?.name ?? '');
    setHostText(manual?.host ?? '');
    setPortText(manual === null ? '' : String(manual.port));
    setEditError({});
    setRenaming(true);
  };

  /** Name, address and port of a computer added by hand (H60). */
  const saveName = async () => {
    if (manual === null || savingEdit) {
      return;
    }
    const trimmed = name.trim();
    let host = hostText.trim();
    let embeddedPort: number | undefined;
    if (hostKind(host) === 'invalid') {
      // `host:port` and `[v6]:port` carry their own port.
      const split = parseHostPort(host, defaultPort);
      if (split === null) {
        setEditError({ host: native.sheet.badHost });
        return;
      }
      embeddedPort = /:\d+$/.test(host) ? split.port : undefined;
      host = split.host;
    }
    const portValue = portText.trim();
    let port = embeddedPort ?? defaultPort;
    if (portValue !== '') {
      const parsed = Number(portValue);
      if (!/^\d+$/.test(portValue) || parsed < 1 || parsed > 65535) {
        setEditError({ port: native.sheet.badPort });
        return;
      }
      port = parsed;
    }
    setEditError({});
    setSavingEdit(true);
    try {
      await manualServers.saveComputer({
        ...manual,
        name: trimmed === '' ? null : trimmed,
        host,
        port,
      });
      setRenaming(false);
      show(native.sheet.saved, { tone: 'good' });
    } catch {
      setRenaming(false);
      show(native.sheet.failure, { tone: 'bad', icon: 'alert' });
    } finally {
      setSavingEdit(false);
    }
  };

  /**
   * Screen Sharing or Remote Desktop for a computer added by hand:
   * a Windows PC with a VNC server may have been saved as a Mac. The system
   * follows the protocol; a default port moves to the new default.
   */
  const changeProtocol = async (protocol: NativeProtocol) => {
    if (manual === null || savingProtocol) {
      return;
    }
    const current = entryProtocol(entry);
    setSavingProtocol(true);
    try {
      await manualServers.saveComputer({
        ...manual,
        os: PROTOCOL_OS[protocol],
        protocol,
        port:
          manual.port === PROTOCOL_PORT[current]
            ? PROTOCOL_PORT[protocol]
            : manual.port,
      });
      show(native.sheet.saved, { tone: 'good' });
    } catch {
      show(native.sheet.failure, { tone: 'bad', icon: 'alert' });
    } finally {
      setSavingProtocol(false);
    }
  };

  const remove = async () => {
    if (manual === null || removing) {
      return;
    }
    setRemoving(true);
    setRemoved(entry);
    try {
      await manualServers.removeComputer(manual.id);
    } catch {
      setRemoved(null);
      setRemoving(false);
      setConfirmingRemove(false);
      show(native.sheet.failure, { tone: 'bad', icon: 'alert' });
      return;
    }
    close();
    // The sheet is gone by now, so the way back is the App-wide toast.
    const { saveComputer } = manualServers;
    showToast({
      message: deviceSheet.native.removed(card.name),
      action: {
        label: deviceSheet.native.undo,
        onPress: () => {
          saveComputer(manual).catch(() =>
            showToast({
              message: deviceSheet.native.undoFailed,
              tone: 'bad',
              icon: 'alert',
            }),
          );
        },
      },
    });
  };

  return (
    <BottomSheet
      leading={
        <View style={styles.leading}>
          <Icon
            color="text2"
            name={card.look === 'offline' ? 'computerOff' : 'computer'}
            size={24}
          />
        </View>
      }
      onClose={close}
      scroll
      subtitle={card.subtitle}
      testID="native-sheet"
      title={card.name}
    >
      <View style={styles.content}>
        <View style={styles.chipRow}>
          {reach === 'unknown' ? null : (
            <StatusChip
              label={card.reachText}
              tone={reach === 'online' ? 'good' : 'offline'}
            />
          )}
          <StatusChip label={card.badge} tone="accent" />
        </View>

        <Button
          disabled={busy}
          fullWidth
          loading={busy}
          onPress={() => {
            connect(entry, { replace: true }).catch(() => undefined);
          }}
          testID="native-sheet-connect"
          title={common.connect}
          variant="primaryLarge"
        />

        <ListGroup>
          <ListRow
            title={native.sheet.os}
            value={native.sheet.osName(card.os)}
          />
          {card.address === '' ? null : (
            <ListRow title={native.sheet.address} value={card.address} />
          )}
          <ListRow
            accessibilityLabel={native.a11y.card([
              native.sheet.reach,
              spokenReach,
            ])}
            title={native.sheet.reach}
            value={card.reachText}
          />
        </ListGroup>

        {manual === null ? null : (
          <>
            <ListGroup>
              <ListRow
                chevron
                onPress={startRename}
                testID="native-sheet-rename"
                subtitle={
                  manual.port === defaultPort
                    ? manual.host
                    : `${manual.host}:${manual.port}`
                }
                title={native.sheet.rename}
                value={card.name}
              />
            </ListGroup>
            <ListGroup>
              <View style={styles.protocolRow}>
                <AppText variant="body">{native.sheet.protocol}</AppText>
                <Segmented
                  accessibilityLabel={native.sheet.protocol}
                  onChange={protocol => {
                    changeProtocol(protocol).catch(() => undefined);
                  }}
                  options={PROTOCOL_OPTIONS.map(option => ({
                    ...option,
                    disabled: savingProtocol,
                  }))}
                  testID="native-sheet-protocol"
                  value={entryProtocol(entry)}
                />
              </View>
            </ListGroup>
            <Button
              align="start"
              onPress={() => setConfirmingRemove(true)}
              testID="native-sheet-remove"
              textColor="bad"
              title={native.sheet.remove}
              variant="text"
            />
          </>
        )}

        <Button
          align="start"
          onPress={() => navigation.replace('PairAdd')}
          testID="native-sheet-install-host"
          title={native.sheet.installHost}
          variant="text"
        />
      </View>

      <Dialog
        actions={[
          {
            label: native.sheet.save,
            loading: savingEdit,
            onPress: () => {
              saveName().catch(() => undefined);
            },
            testID: 'native-sheet-save',
          },
          {
            label: common.cancel,
            onPress: () => setRenaming(false),
            variant: 'text',
          },
        ]}
        onDismiss={() => setRenaming(false)}
        testID="native-sheet-rename-dialog"
        title={native.sheet.editTitle}
        visible={renaming}
      >
        <View style={styles.fields}>
          <TextField
            accessibilityLabel={native.sheet.nameLabel}
            blurOnSubmit={false}
            label={native.sheet.nameLabel}
            onChangeText={setName}
            onSubmitEditing={() => hostRef.current?.focus()}
            returnKeyType="next"
            testID="native-sheet-name"
            value={name}
          />
          <TextField
            accessibilityLabel={native.sheet.hostLabel}
            autoCapitalize="none"
            autoCorrect={false}
            blurOnSubmit={false}
            error={editError.host}
            keyboardType="url"
            label={native.sheet.hostLabel}
            mono
            onChangeText={value => {
              setHostText(value);
              setEditError({});
            }}
            onSubmitEditing={() => portRef.current?.focus()}
            ref={hostRef}
            returnKeyType="next"
            testID="native-sheet-host"
            value={hostText}
          />
          <TextField
            accessibilityLabel={native.sheet.portLabel}
            error={editError.port}
            keyboardType="number-pad"
            label={native.sheet.portLabel}
            maxLength={5}
            mono
            onChangeText={value => {
              setPortText(value);
              setEditError({});
            }}
            onSubmitEditing={() => {
              saveName().catch(() => undefined);
            }}
            placeholder={native.sheet.portPlaceholder(defaultPort)}
            ref={portRef}
            returnKeyType="done"
            testID="native-sheet-port"
            value={portText}
          />
        </View>
      </Dialog>
      <Dialog
        actions={[
          {
            label: native.sheet.removeConfirm,
            loading: removing,
            onPress: () => {
              remove().catch(() => undefined);
            },
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setConfirmingRemove(false),
            variant: 'secondary',
          },
        ]}
        message={native.sheet.removeMessage}
        onDismiss={() => setConfirmingRemove(false)}
        testID="native-sheet-remove-dialog"
        title={native.sheet.removeTitle(card.name)}
        visible={confirmingRemove}
      />
      <FallbackGuide guide={guide} onClose={closeGuide} />
      <Toast toast={toast} onHide={hide} position="bottom" />
    </BottomSheet>
  );
}

const styles = StyleSheet.create({
  leading: {
    width: 48,
    height: 48,
    borderRadius: 14,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  content: {
    gap: 14,
  },
  chipRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 8,
  },
  fields: {
    gap: 12,
  },
  protocolRow: {
    gap: 8,
    paddingHorizontal: 14,
    paddingVertical: 12,
  },
  message: {
    minHeight: 140,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
