import { useState } from 'react';
import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import BottomSheet from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { StatusChip } from '../../components/StatusChip';
import { Switch } from '../../components/Switch';
import { Toast, useToast } from '../../components/Toast';
import { common } from '../../i18n/zh/common';
import { deviceSheet } from '../../i18n/zh/deviceSheet';
import { native } from '../../i18n/zh/native';
import { useAuthName } from '../../features/lock/authName';
import { AdapterEntryRow } from '../../features/adapters/AdapterEntryRow';
import { adaptersAvailable } from '../../features/adapters/logic';
import { FallbackGuide } from '../../features/fallback/FallbackGuide';
import type { ComputerEntry } from '../../features/native/types';
import { useComputerEntries } from '../../features/native/useComputerEntries';
import { decideRoute, useConnect } from '../../features/native/useConnect';
import { displayName } from '../../features/directory/displayLabel';
import { useSetting } from '../../features/settings/store';
import { useCore } from '../../native/rscore/CoreContext';
import { tokens } from '../../theme/tokens';
import type { RootScreenProps } from '../../app/navigation/routes';
import { ConnectViaPicker, connectViaLabel } from './ConnectViaPicker';
import DisplayPicker from './DisplayPicker';
import QuickActions from './QuickActions';
import { useDeviceSheet } from './useDeviceSheet';

type HostEntry = Extract<ComputerEntry, { kind: 'host' }>;

export default function DeviceSheetScreen({
  navigation,
  route,
}: RootScreenProps<'DeviceSheet'>) {
  const { hostId } = route.params;
  const sheet = useDeviceSheet(hostId);
  const { toast, show, hide } = useToast();
  const [pairDialogVisible, setPairDialogVisible] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const [viaPickerVisible, setViaPickerVisible] = useState(false);
  const authenticationName = useAuthName();
  const core = useCore();
  const [connectPrefer] = useSetting('connect.prefer');
  const entry = useComputerEntries().find(
    (candidate): candidate is HostEntry =>
      candidate.kind === 'host' && candidate.computer.hostId === hostId,
  );
  const { connect, busy, guide, closeGuide } = useConnect();

  const close = () => navigation.goBack();

  if (sheet.notFound) {
    return (
      <BottomSheet onClose={close} title={deviceSheet.notFound.title}>
        <View style={styles.notFound}>
          <Button
            fullWidth
            onPress={close}
            title={common.close}
            variant="secondary"
          />
        </View>
      </BottomSheet>
    );
  }

  if (sheet.host === undefined) {
    return (
      <BottomSheet onClose={close} title={deviceSheet.title}>
        <View style={styles.loading}>
          <AppText color="text3">{common.loading}</AppText>
        </View>
      </BottomSheet>
    );
  }

  const host = sheet.host;
  const hasMultipleDisplays = sheet.displays.length > 1;
  // Nothing reaches an offline computer and it cannot be woken from here.
  const offline = !host.online && sheet.liveSession === undefined;
  const primaryLabel = offline
    ? deviceSheet.offline.button
    : sheet.selectedDisplay === undefined
    ? deviceSheet.displays.start
    : hasMultipleDisplays
    ? deviceSheet.displays.control(
        displayName(
          sheet.selectedDisplay,
          sheet.displays.indexOf(sheet.selectedDisplay) + 1,
        ),
      )
    : deviceSheet.displays.start;
  const openSession = (viewOnly: boolean) =>
    navigation.replace('Session', {
      hostId,
      ...(viewOnly ? { viewOnly: true } : null),
      ...(sheet.selectedDisplay === undefined
        ? null
        : { displayId: sheet.selectedDisplay.id }),
    });
  const start = () => {
    // A Host session keeps the chosen display and returns to a live session;
    // everything else is the native-first route (C7).
    if (
      entry === undefined ||
      sheet.liveSession !== undefined ||
      decideRoute({
        entry,
        adaptersAvailable: adaptersAvailable(core),
        connectPrefer,
        hostPrefs: { connectVia: sheet.connectVia },
        discoverAvailable: core.discover !== undefined,
      }).kind === 'session'
    ) {
      openSession(false);
      return;
    }
    connect(entry, { replace: true }).catch(() => undefined);
  };

  const confirmForget = async () => {
    if (forgetting) {
      return;
    }
    setForgetting(true);
    const forgotten = await sheet.forgetHost();
    setForgetting(false);
    setPairDialogVisible(false);
    if (!forgotten) {
      show(deviceSheet.pair.failure, { tone: 'bad', icon: 'alert' });
      return;
    }
    close();
  };

  const lock = () => {
    if (sheet.lockSession()) {
      show(deviceSheet.quickActions.locked, { tone: 'good' });
    }
  };

  const lockUnavailable = () =>
    show(deviceSheet.a11y.lockNeedsSession, { icon: 'lock' });

  return (
    <BottomSheet
      leading={
        <View style={styles.leading}>
          <Icon color="text2" name="computer" size={24} />
        </View>
      }
      onClose={close}
      scroll
      subtitle={sheet.subtitle}
      testID="device-sheet"
      title={host.name}
    >
      <View style={styles.content}>
        <View style={styles.chipRow}>
          <StatusChip
            label={sheet.pathChip.label}
            mark={sheet.pathChip.mark}
            tone={sheet.pathChip.tone}
          />
          {host.osVersion === '' ? null : (
            <StatusChip label={host.osVersion} tone="neutral" />
          )}
          <StatusChip
            label={`${deviceSheet.chips.host} ${host.hostVersion}`}
            tone="neutral"
          />
        </View>

        <DisplayPicker
          displays={sheet.displays}
          onSelect={sheet.selectDisplay}
          selectedId={sheet.selectedDisplay?.id}
        />

        <Button
          disabled={busy || offline}
          fullWidth
          loading={busy}
          onPress={start}
          testID="device-sheet-start"
          title={primaryLabel}
          variant="primaryLarge"
        />
        {offline ? (
          <AppText
            color="text3"
            testID="device-sheet-offline-reason"
            variant="footnote"
          >
            {deviceSheet.offline.reason}
          </AppText>
        ) : null}

        <QuickActions
          canLock={sheet.liveSession !== undefined}
          offline={offline}
          onLock={lock}
          onLockUnavailable={lockUnavailable}
          onTerminal={() =>
            navigation.replace('Terminal', { serverId: host.node })
          }
          onViewOnly={() => openSession(true)}
        />

        <AdapterEntryRow
          hostId={hostId}
          onOpen={params => navigation.replace('AdapterSession', params)}
        />

        <ListGroup>
          <ListRow
            chevron
            onPress={() => setViaPickerVisible(true)}
            testID="connect-via-row"
            title={native.connectVia.title}
            value={connectViaLabel(sheet.connectVia, host.os, connectPrefer)}
          />
          <ListRow
            chevron
            onPress={() => navigation.navigate('DeviceRoutes', { hostId })}
            title={deviceSheet.rows.routes}
            value={sheet.addressSummary}
          />
          {sheet.lastConnectionText === null ? null : (
            <ListRow
              title={deviceSheet.rows.lastConnection}
              value={sheet.lastConnectionText}
            />
          )}
        </ListGroup>

        <ListGroup>
          <ListRow
            right={
              <Switch
                accessibilityLabel={deviceSheet.switches.sensitive(
                  authenticationName,
                )}
                onColor={tokens.accent}
                onValueChange={sheet.setSensitive}
                testID="switch-sensitive"
                value={sheet.sensitive}
              />
            }
            title={deviceSheet.switches.sensitive(authenticationName)}
          />
          <ListRow
            right={
              <Switch
                accessibilityLabel={deviceSheet.switches.lockOnEnd}
                onValueChange={sheet.setLockOnEnd}
                testID="switch-lock-on-end"
                value={sheet.lockOnEnd}
              />
            }
            title={deviceSheet.switches.lockOnEnd}
          />
        </ListGroup>

        <Button
          accessibilityLabel={deviceSheet.pair.cancel}
          align="start"
          onPress={() => setPairDialogVisible(true)}
          testID="device-sheet-forget"
          title={deviceSheet.pair.cancel}
          textColor="bad"
          variant="text"
        />
      </View>

      <Dialog
        actions={[
          {
            label: deviceSheet.pair.confirm,
            loading: forgetting,
            onPress: () => {
              confirmForget().catch(() => undefined);
            },
            testID: 'forget-confirm',
            variant: 'pillDanger',
          },
          {
            label: deviceSheet.pair.keep,
            onPress: () => setPairDialogVisible(false),
            testID: 'forget-keep',
            variant: 'secondary',
          },
        ]}
        message={deviceSheet.pair.message}
        onDismiss={() => setPairDialogVisible(false)}
        testID="forget-dialog"
        title={deviceSheet.pair.title(host.name)}
        visible={pairDialogVisible}
      />
      <ConnectViaPicker
        onChange={via => {
          sheet.setConnectVia(via);
          setViaPickerVisible(false);
        }}
        onClose={() => setViaPickerVisible(false)}
        os={host.os}
        prefer={connectPrefer}
        value={sheet.connectVia}
        visible={viaPickerVisible}
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
  loading: {
    minHeight: 140,
    alignItems: 'center',
    justifyContent: 'center',
  },
  notFound: {
    paddingVertical: 8,
  },
});
