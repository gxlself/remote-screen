import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Keyboard,
  Platform,
  PixelRatio,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { FormScroll } from '../../components/FormScroll';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { TextField } from '../../components/TextField';
import type { TextFieldHandle } from '../../components/TextField';
import { showToast } from '../../components/ToastHost';
import { useNetwork } from '../../features/events/EventsProvider';
import { FallbackGuide } from '../../features/fallback/FallbackGuide';
import { useRemoteFallback } from '../../features/fallback/useRemoteFallback';
import { useDirectory } from '../../features/data/queries';
import { useConnect } from '../../features/native/useConnect';
import { judgeNetwork } from '../../features/network/judge';
import type { NetVerdict } from '../../features/network/judge';
import { hostKind, requiredNetwork } from '../../features/ssh/serverForm';
import type { ManualComputer } from '../../features/servers/manual';
import { newComputerId } from '../../features/servers/manual';
import {
  RDP_PORT,
  VNC_PORT,
  addrVerdict,
  matchComputer,
  nativeOffers,
  parseAddressForm,
  sharingOff,
} from '../../features/servers/addressProbe';
import type {
  ComputerMatch,
  NativeOffer,
  NativeOs,
  ProbeHello,
} from '../../features/servers/addressProbe';
import { useManualServers } from '../../features/servers/useManualServers';
import { useSetting } from '../../features/settings/store';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { addByAddress } from '../../i18n/zh/addByAddress';
import { common } from '../../i18n/zh/common';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { space, tokens } from '../../theme/tokens';
import { ProbeResultCard } from './ProbeResultCard';
import { useAddressProbe } from './useAddressProbe';

type FormError = 'badHost' | 'badPort' | null;

function isPrivateHost(host: string): boolean {
  return hostKind(host) === 'private';
}

function networkLine(
  verdict: NetVerdict,
  tailnetEnabled: boolean,
): string | null {
  if (verdict.companyVpn !== null) {
    return addByAddress.network.vpn(verdict.companyVpn.addr);
  }
  if (verdict.atCompany !== null) {
    return addByAddress.network.intranet;
  }
  if (
    tailnetEnabled &&
    verdict.tailnet === 'up' &&
    verdict.tailnetAddr !== null
  ) {
    return addByAddress.network.tailnet(verdict.tailnetAddr);
  }
  return null;
}

function elapsedText(elapsedMs: number): string {
  return (elapsedMs / 1000).toFixed(1);
}

function knownCopy(match: ComputerMatch): string {
  const name = match.computer.name;
  switch (match.by) {
    case 'fp':
      return addByAddress.known.fp(name);
    case 'hostId':
      return addByAddress.known.hostId(name);
    case 'address':
      return addByAddress.known.address(name);
  }
}

function displayName(match: ComputerMatch, hello: ProbeHello | null): string {
  return match.computer.name || hello?.name || match.computer.node;
}

/** A name the probe resolved for the address, else the address itself. */
function sshServerName(
  host: string,
  match: ComputerMatch | null,
  hello: ProbeHello | null,
): string {
  return match?.computer.name || hello?.name || host;
}

export default function AddByAddressScreen({
  navigation,
}: RootScreenProps<'AddByAddress'>) {
  const insets = useSafeAreaInsets();
  const fallback = useRemoteFallback();
  const { probe, start, reset } = useAddressProbe();
  const { directory } = useDirectory();
  const { computers, saveComputer, removeComputer } = useManualServers();
  const {
    connect,
    busy: connecting,
    guide: connectGuide,
    closeGuide: closeConnectGuide,
  } = useConnect();
  const [networks] = useSetting('networks');
  const tailnetEnabled = useTailnetEnabled();
  const network = useNetwork(state => state.status);
  const [host, setHost] = useState('');
  const [portText, setPortText] = useState('');
  const [formError, setFormError] = useState<FormError>(null);
  const [submitted, setSubmitted] = useState<{
    host: string;
    port: number;
  } | null>(null);
  const [asNew, setAsNew] = useState(false);
  const [saving, setSaving] = useState(false);
  const [pressedOffer, setPressedOffer] = useState<NativeOffer['protocol'] | null>(
    null,
  );
  const [saveError, setSaveError] = useState<string | null>(null);
  const savingRef = useRef(false);
  const portRef = useRef<TextFieldHandle>(null);
  /** Saved for 输入配对码; removed again unless the pairing finishes. */
  const pendingPairId = useRef<string | null>(null);
  /** Back from the sharing guide: look again, sharing may be on now. */
  const recheck = useRef(false);
  const largeText = PixelRatio.getFontScale() > 2;

  useEffect(() => {
    if (saveError !== null && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(saveError);
    }
  }, [saveError]);

  const match = useMemo(
    () =>
      matchComputer(
        submitted?.host ?? '',
        directory?.computers ?? [],
        probe.hello,
      ),
    [directory?.computers, probe.hello, submitted?.host],
  );
  const effectiveVerdict = addrVerdict(probe, match);
  const activeComputer = match?.computer ?? null;
  const targetPort = submitted?.port ?? 7400;
  const netVerdict = judgeNetwork(network, networks);
  const netText = networkLine(netVerdict, tailnetEnabled);
  const elapsed =
    probe.state === 'done' && probe.elapsedMs !== null
      ? elapsedText(probe.elapsedMs)
      : null;
  const showPairing =
    asNew ||
    effectiveVerdict === 'new' ||
    (effectiveVerdict === 'known' && match?.by === 'hostId');

  const search = () => {
    setSaveError(null);
    setAsNew(false);
    const parsed = parseAddressForm(host, portText);
    if ('error' in parsed) {
      setFormError(parsed.error);
      return;
    }
    Keyboard.dismiss();
    setFormError(null);
    setSubmitted({ host: parsed.host, port: parsed.port });
    start(parsed.host, parsed.port);
  };
  const searchRef = useRef(search);
  searchRef.current = search;

  // The results belong to the address that was searched; once it is edited
  // they would describe another computer.
  const clearResult = () => {
    if (submitted === null && probe.state === 'idle') {
      return;
    }
    reset();
    setSubmitted(null);
    setAsNew(false);
    setSaveError(null);
  };

  useEffect(
    () =>
      navigation.addListener('focus', () => {
        const pairId = pendingPairId.current;
        if (pairId !== null) {
          // A finished pairing resets the stack to Home; coming back here
          // means it was cancelled or failed.
          pendingPairId.current = null;
          removeComputer(pairId).catch(() => undefined);
        }
        if (recheck.current) {
          recheck.current = false;
          searchRef.current();
        }
      }),
    [navigation, removeComputer],
  );

  const goPair = (
    params: { tab: 'code'; addr: string; port: number } | { tab: 'scan' },
  ) => {
    navigation.navigate('PairAdd', params);
  };

  const addSshServer = () => {
    if (submitted === null) {
      return;
    }
    navigation.replace('AddServer', {
      prefill: {
        name: sshServerName(submitted.host, match, probe.hello),
        host: submitted.host,
        port: 22,
        connectAfterSave: true,
      },
    });
  };

  const saveSecondAddress = async () => {
    if (
      match === null ||
      submitted === null ||
      savingRef.current ||
      match.by === 'hostId'
    ) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    const name = displayName(match, probe.hello);
    const entry: ManualComputer = {
      id: newComputerId(
        submitted.host,
        submitted.port,
        computers.map(computer => computer.id),
      ),
      kind: 'computer',
      host: submitted.host,
      port: submitted.port,
      hostId: match.computer.hostId,
      name,
      networkId: requiredNetwork(submitted.host, networks)?.id ?? null,
      addedAtUs: Date.now() * 1000,
    };
    try {
      await saveComputer(entry);
      showToast({ message: addByAddress.actions.added, tone: 'good' });
      navigation.goBack();
    } catch {
      setSaveError(addByAddress.actions.saveFailed);
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const saveThenPair = async (hello: ProbeHello) => {
    if (submitted === null || savingRef.current) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    const entry: ManualComputer = {
      id: newComputerId(
        submitted.host,
        submitted.port,
        computers.map(computer => computer.id),
      ),
      kind: 'computer',
      host: submitted.host,
      port: submitted.port,
      hostId: hello.hostId,
      name: hello.name ?? null,
      networkId: requiredNetwork(submitted.host, networks)?.id ?? null,
      addedAtUs: Date.now() * 1000,
    };
    try {
      await saveComputer(entry);
      pendingPairId.current = entry.id;
      goPair({ tab: 'code', addr: submitted.host, port: submitted.port });
    } catch {
      setSaveError(addByAddress.actions.saveFailed);
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  /** A computer without a Host, kept under the same id when re-added. */
  const nativeEntry = (
    target: { host: string },
    os: NativeOs,
    protocol: NativeOffer['protocol'],
    port: number,
    name: string | null,
  ): ManualComputer & { os: NativeOs } => {
    const existing = computers.find(
      computer =>
        computer.hostId === null &&
        computer.host === target.host &&
        computer.protocol === protocol,
    );
    return {
      id:
        existing?.id ??
        newComputerId(
          target.host,
          port,
          computers.map(computer => computer.id),
        ),
      kind: 'computer',
      host: target.host,
      port,
      hostId: null,
      name: existing?.name ?? name,
      networkId: requiredNetwork(target.host, networks)?.id ?? null,
      addedAtUs: existing?.addedAtUs ?? Date.now() * 1000,
      os,
      protocol,
    };
  };

  const saveThenConnect = async (offer: NativeOffer) => {
    if (submitted === null || savingRef.current) {
      return;
    }
    // One busy state from the tap until the session opens: saving, then the
    // connect probe.
    savingRef.current = true;
    setSaving(true);
    setPressedOffer(offer.protocol);
    setSaveError(null);
    const entry = nativeEntry(
      submitted,
      offer.os,
      offer.protocol,
      offer.port,
      match?.computer.name ?? probe.hello?.name ?? null,
    );
    try {
      await saveComputer(entry);
    } catch {
      setSaveError(addByAddress.actions.saveFailed);
      savingRef.current = false;
      setSaving(false);
      setPressedOffer(null);
      return;
    }
    try {
      // The session replaces this page: ending it goes back to where adding
      // started, not to these results.
      await connect(
        { kind: 'manual', key: `manual:${entry.id}`, manual: entry },
        { replace: true },
      );
    } finally {
      savingRef.current = false;
      setSaving(false);
      setPressedOffer(null);
    }
  };

  const addAnyway = async (os: NativeOs) => {
    if (submitted === null || savingRef.current) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    const entry =
      os === 'macos'
        ? nativeEntry(submitted, 'macos', 'vnc', VNC_PORT, null)
        : nativeEntry(submitted, 'windows', 'rdp', RDP_PORT, null);
    try {
      await saveComputer(entry);
      showToast({ message: addByAddress.actions.added, tone: 'good' });
      navigation.goBack();
    } catch {
      setSaveError(addByAddress.actions.saveFailed);
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const offers = nativeOffers(probe);
  const offSystems = sharingOff(
    probe,
    match?.computer.os ?? probe.hello?.os ?? null,
  );
  const isKnown = effectiveVerdict === 'known' && match !== null;
  const isNotFound = effectiveVerdict === 'notFound';
  // The external-app row is the pre-V10 path; with discover the Remote
  // Desktop button above replaces it.
  const rdpOpen =
    !probe.discover && probe.state === 'done' && probe.rdp === 'open';
  const sshOpen =
    probe.state === 'done' && probe.ssh === 'open' && submitted !== null;
  // A typical Linux server: no Remote Host, no Screen Sharing or Remote
  // Desktop, only sshd.
  const sshOnly =
    sshOpen &&
    isNotFound &&
    match === null &&
    probe.vnc !== 'open' &&
    probe.rdp !== 'open';
  const reachable =
    offers.length > 0 ? offers[0].protocol : sshOnly ? 'ssh' : null;
  // Nothing answered that this page can connect to now; the computer may be
  // asleep or its sharing still off.
  const unreachable =
    probe.state === 'done' &&
    submitted !== null &&
    isNotFound &&
    offers.length === 0 &&
    !sshOnly &&
    !rdpOpen;
  const anywaySystems: NativeOs[] =
    offSystems.length > 0 ? offSystems : ['macos', 'windows'];
  const busy = saving || connecting;
  const privateAddress =
    submitted === null ? true : isPrivateHost(submitted.host);
  const addLabel =
    isKnown && match !== null
      ? addByAddress.actions.addAddress(
          privateAddress
            ? addByAddress.actions.intranetAddress
            : addByAddress.actions.connectionAddress,
          displayName(match, probe.hello),
        )
      : '';
  const knownNote = isKnown && match !== null ? knownCopy(match) : null;
  const statusLine = [netText, elapsed === null ? null : addByAddress.network.elapsed(elapsed)]
    .filter((part): part is string => part !== null)
    .join(' · ');

  return (
    <FormScroll
      contentContainerStyle={[
        styles.content,
        { paddingBottom: insets.bottom + 24 },
      ]}
      footer={
        <>
          <FallbackGuide guide={fallback.guide} onClose={fallback.closeGuide} />
          <FallbackGuide guide={connectGuide} onClose={closeConnectGuide} />
        </>
      }
      header={
        <View style={[styles.header, { paddingTop: insets.top + 8 }]}>
          <IconButton
            accessibilityLabel={common.a11y.back}
            icon="chevronLeft"
            onPress={() => navigation.goBack()}
            size={44}
            variant="raised"
          />
        </View>
      }
      style={styles.screen}
      testID="add-by-address-form"
    >
      <AppText accessibilityRole="header" variant="title" weight={700}>
        {addByAddress.title}
      </AppText>

      <View style={styles.form}>
        <View
          style={[styles.addressRow, largeText ? styles.addressRowLarge : null]}
        >
          <View
            style={[styles.hostField, largeText ? styles.hostFieldLarge : null]}
          >
            <TextField
              accessibilityLabel={addByAddress.a11y.address}
              autoComplete="off"
              blurOnSubmit={false}
              value={host}
              onChangeText={value => {
                setHost(value);
                clearResult();
                if (formError === 'badHost') {
                  setFormError(null);
                }
              }}
              error={
                formError === 'badHost'
                  ? addByAddress.errors.badHost
                  : undefined
              }
              keyboardType="url"
              label={addByAddress.addressLabel}
              mono
              onSubmitEditing={() => portRef.current?.focus()}
              returnKeyType="next"
              testID="add-address-host"
              textContentType="URL"
            />
          </View>
          <View
            style={[styles.portField, largeText ? styles.portFieldLarge : null]}
          >
            <TextField
              ref={portRef}
              accessibilityLabel={addByAddress.a11y.port}
              value={portText}
              onChangeText={value => {
                setPortText(value);
                clearResult();
                if (formError === 'badPort') {
                  setFormError(null);
                }
              }}
              error={
                formError === 'badPort'
                  ? addByAddress.errors.badPort
                  : undefined
              }
              label={addByAddress.portLabel}
              maxLength={5}
              mono
              onSubmitEditing={search}
              placeholder={addByAddress.portPlaceholder}
              keyboardType="number-pad"
              returnKeyType="done"
              testID="add-address-port"
            />
          </View>
        </View>
        <Button
          accessibilityLabel={addByAddress.a11y.find}
          fullWidth
          loading={probe.state === 'running'}
          onPress={search}
          testID="add-address-find"
          title={addByAddress.find}
        />
      </View>

      {statusLine === '' ? null : (
        <AppText color="text3" testID="add-address-status" variant="footnote">
          {statusLine}
        </AppText>
      )}

      <ProbeResultCard
        match={match}
        port={targetPort}
        probe={probe}
        reachable={reachable}
        verdict={effectiveVerdict}
      />

      {offers.length === 0 ? null : (
        <View style={styles.actions}>
          {offers.map((offer, index) => (
            <Button
              key={offer.protocol}
              disabled={busy}
              fullWidth
              loading={busy && pressedOffer === offer.protocol}
              onPress={() => {
                saveThenConnect(offer);
              }}
              testID={`add-address-connect-${offer.protocol}`}
              title={
                offer.protocol === 'vnc'
                  ? addByAddress.native.connectVnc
                  : addByAddress.native.connectRdp
              }
              variant={index === 0 ? 'primary' : 'secondary'}
            />
          ))}
        </View>
      )}

      {sshOnly ? (
        <View style={styles.actions}>
          <Button
            fullWidth
            onPress={addSshServer}
            testID="add-address-ssh"
            title={addByAddress.actions.addSsh}
          />
        </View>
      ) : null}

      {offSystems.length === 0 ? null : (
        <ListGroup>
          {offSystems.map(os => (
            <ListRow
              key={os}
              chevron
              onPress={() => {
                recheck.current = true;
                navigation.navigate('EnableSharing', { os });
              }}
              title={addByAddress.native.off(os)}
              value={addByAddress.native.howTo}
            />
          ))}
        </ListGroup>
      )}

      {knownNote === null ? null : (
        <AppText
          color={match?.by === 'hostId' ? 'relay' : 'text2'}
          variant="footnote"
        >
          {knownNote}
        </AppText>
      )}

      {probe.state === 'done' ? (
        <View style={styles.section}>
          <SectionHeader title={addByAddress.services.title} />
          <ListGroup>
            {probe.vnc === null ? null : (
              <ListRow
                subtitle={addByAddress.services.vncSubtitle}
                title={addByAddress.services.vnc}
                value={
                  probe.vnc === 'open'
                    ? addByAddress.services.open
                    : addByAddress.services.closed
                }
              />
            )}
            <ListRow
              subtitle={addByAddress.services.rdpSubtitle}
              title={addByAddress.services.rdp}
              value={
                probe.rdp === 'open'
                  ? addByAddress.services.open
                  : addByAddress.services.closed
              }
            />
            {sshOpen ? (
              <ListRow
                chevron
                onPress={addSshServer}
                testID="add-address-ssh-row"
                title={addByAddress.services.ssh}
                value={addByAddress.actions.addSsh}
              />
            ) : (
              <ListRow
                title={addByAddress.services.ssh}
                value={addByAddress.services.noResponse}
              />
            )}
            {isNotFound && rdpOpen && submitted !== null ? (
              <ListRow
                chevron
                onPress={() => {
                  fallback
                    .open({
                      os: 'windows',
                      addr: submitted.host,
                      port: 3389,
                    })
                    .then(result => {
                      if (result === 'failed') {
                        setSaveError(sessionEnded.screenShareUnavailable);
                      }
                    })
                    .catch(() => undefined);
                }}
                title={addByAddress.services.openRdp}
              />
            ) : null}
          </ListGroup>
        </View>
      ) : null}

      {unreachable ? (
        <View style={styles.section} testID="add-address-anyway">
          <SectionHeader title={addByAddress.addAnyway.title} />
          <AppText color="text2" variant="footnote">
            {addByAddress.addAnyway.note}
          </AppText>
          <View style={styles.actions}>
            {anywaySystems.map(os => (
              <Button
                key={os}
                disabled={busy}
                fullWidth
                onPress={() => {
                  addAnyway(os);
                }}
                testID={`add-address-anyway-${os}`}
                title={
                  os === 'macos'
                    ? addByAddress.addAnyway.mac
                    : addByAddress.addAnyway.windows
                }
                variant="secondary"
              />
            ))}
          </View>
        </View>
      ) : null}

      {saveError === null ? null : (
        <AppText color="bad" testID="add-address-error" variant="footnote">
          {saveError}
        </AppText>
      )}

      {isKnown && match !== null && match.by !== 'hostId' ? (
        <View style={styles.actions}>
          <Button
            disabled={busy}
            fullWidth
            loading={saving}
            onPress={() => {
              saveSecondAddress();
            }}
            testID="add-address-save"
            title={addLabel}
          />
          <Button
            align="start"
            onPress={() => setAsNew(current => !current)}
            title={
              asNew
                ? addByAddress.actions.keepSecondAddress
                : addByAddress.actions.pairAsNew
            }
            variant="text"
          />
        </View>
      ) : null}

      {showPairing ? (
        <View style={styles.section}>
          <AppText color="text2" variant="footnote">
            {isKnown && activeComputer !== null
              ? addByAddress.pair.instructionsKnown(activeComputer.name)
              : addByAddress.pair.instructionsNew}
          </AppText>
          {probe.hello === null ? (
            <>
              <AppText color="text3" variant="footnote">
                {addByAddress.pair.missingHello}
              </AppText>
              <Button
                align="start"
                onPress={() => goPair({ tab: 'scan' })}
                title={addByAddress.pair.scan}
                variant="secondary"
              />
            </>
          ) : (
            <ListGroup>
              <ListRow
                chevron
                disabled={busy}
                onPress={() => {
                  if (probe.hello !== null) {
                    saveThenPair(probe.hello);
                  }
                }}
                subtitle={addByAddress.pair.codeSubtitle}
                title={addByAddress.pair.code}
              />
            </ListGroup>
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
  content: {
    paddingHorizontal: space.gutter,
    gap: space.md,
  },
  header: {
    paddingHorizontal: space.gutter,
    paddingBottom: space.sm,
  },
  form: {
    gap: space.sm,
  },
  addressRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: space.sm,
  },
  addressRowLarge: {
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
    width: 110,
  },
  portFieldLarge: {
    width: undefined,
  },
  section: {
    gap: space.sm,
  },
  actions: {
    gap: space.xs,
  },
});
