import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Linking, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { FormScroll } from '../../components/FormScroll';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { TextField } from '../../components/TextField';
import { addByAddress } from '../../i18n/zh/addByAddress';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import {
  bonjourAvailable,
  browseScreenSharing,
} from '../../native/bonjour/bonjour';
import { useCore } from '../../native/rscore/CoreContext';
import type { CoreEventOf } from '../../native/rscore/types';
import type {
  BonjourEvent,
  BonjourService,
} from '../../native/specs/NativeRsBonjour';
import { space, tokens } from '../../theme/tokens';
import { useCoreEvent } from '../events/EventsProvider';
import { FallbackGuide } from '../fallback/FallbackGuide';
import { RDP_PORT, VNC_PORT } from '../servers/addressProbe';
import { newComputerId } from '../servers/manual';
import type { ManualComputer } from '../servers/manual';
import { useManualServers } from '../servers/useManualServers';
import { useSetting } from '../settings/store';
import { requiredNetwork } from '../ssh/serverForm';
import type { ComputerEntry } from './types';
import { useComputerEntries } from './useComputerEntries';
import { useConnect } from './useConnect';

type NativeOs = 'macos' | 'windows';

type Candidate = {
  addr: string;
  port: number;
  os: NativeOs;
  protocol: 'vnc' | 'rdp';
  name: string;
};

type LanHit = { addr: string; ports: number[] };

type ScanEvent = CoreEventOf<'discover.found'> | CoreEventOf<'discover.done'>;

/** `id` is null until `scanLan` resolves; events seen before that wait in `early`. */
type ScanRun = { id: string | null; early: ScanEvent[] };

type ScanState = {
  phase: 'idle' | 'running' | 'done' | 'failed';
  hits: LanHit[];
};

const SCAN_PORTS = [VNC_PORT, RDP_PORT];
/** How long Bonjour may stay silent before the page says nothing was found. */
export const BONJOUR_WAIT_MS = 4000;

function applyBonjour(
  services: BonjourService[],
  event: BonjourEvent,
): BonjourService[] {
  const index = services.findIndex(
    service => service.name === event.service.name,
  );
  if (event.kind === 'lost') {
    return index === -1 ? services : services.filter((_, i) => i !== index);
  }
  if (event.kind !== 'found') {
    return services;
  }
  return index === -1
    ? [...services, event.service]
    : services.map((service, i) => (i === index ? event.service : service));
}

function bonjourAddr(service: BonjourService): string | null {
  return service.addrs[0] ?? (service.host === '' ? null : service.host);
}

function addHit(hits: LanHit[], addr: string, port: number): LanHit[] {
  const index = hits.findIndex(hit => hit.addr === addr);
  if (index === -1) {
    return [...hits, { addr, ports: [port] }];
  }
  if (hits[index].ports.includes(port)) {
    return hits;
  }
  return hits.map((hit, i) =>
    i === index ? { ...hit, ports: [...hit.ports, port] } : hit,
  );
}

/**
 * 3389 means a Windows PC (Remote Desktop), even when 5900 answers too: that
 * is a VNC server on Windows, and a Mac never listens on 3389.
 */
function hitCandidate(hit: LanHit): Candidate {
  return hit.ports.includes(RDP_PORT)
    ? {
        addr: hit.addr,
        port: RDP_PORT,
        os: 'windows',
        protocol: 'rdp',
        name: hit.addr,
      }
    : {
        addr: hit.addr,
        port: VNC_PORT,
        os: 'macos',
        protocol: 'vnc',
        name: hit.addr,
      };
}

/** Each known address, mapped to the computer that already has it. */
function entryAddresses(entries: ComputerEntry[]): Map<string, ComputerEntry> {
  const addrs = new Map<string, ComputerEntry>();
  const add = (addr: string, entry: ComputerEntry) => {
    if (!addrs.has(addr)) {
      addrs.set(addr, entry);
    }
  };
  for (const entry of entries) {
    switch (entry.kind) {
      case 'host':
        entry.computer.addresses.forEach(address => add(address.addr, entry));
        break;
      case 'tailnet':
        entry.device.addrs.forEach(addr => add(addr, entry));
        break;
      case 'manual':
        add(entry.manual.host, entry);
        break;
    }
  }
  return addrs;
}

function scanProgress(scan: ScanState): string | null {
  const found = native.add.scanFound(scan.hits.length);
  switch (scan.phase) {
    case 'idle':
      return null;
    case 'running':
      return scan.hits.length === 0
        ? native.add.scanning
        : `${native.add.scanning} ${found}`;
    case 'done':
      return scan.hits.length === 0
        ? `${found}。${native.add.scanEmptyHint}`
        : found;
    case 'failed':
      return addByAddress.lan.scanFailed;
  }
}

export default function AddComputerScreen({
  navigation,
}: RootScreenProps<'AddComputer'>) {
  const insets = useSafeAreaInsets();
  const core = useCore();
  const discover = core.discover;
  const showBonjour = bonjourAvailable();
  const entries = useComputerEntries();
  const { computers, saveComputer } = useManualServers();
  const [networks] = useSetting('networks');
  const { connect, busy: connecting, guide, closeGuide } = useConnect();
  const [macs, setMacs] = useState<BonjourService[]>([]);
  const [scan, setScan] = useState<ScanState>({ phase: 'idle', hits: [] });
  const [pending, setPending] = useState<Candidate | null>(null);
  const [name, setName] = useState('');
  const [adding, setAdding] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [bonjourWaited, setBonjourWaited] = useState(false);
  const scanRun = useRef<ScanRun | null>(null);
  const savingRef = useRef(false);
  const added = useMemo(() => entryAddresses(entries), [entries]);

  useEffect(() => {
    if (!showBonjour) {
      return undefined;
    }
    const timer = setTimeout(() => setBonjourWaited(true), BONJOUR_WAIT_MS);
    const stop = browseScreenSharing(event =>
      setMacs(current => applyBonjour(current, event)),
    );
    return () => {
      clearTimeout(timer);
      stop();
    };
  }, [showBonjour]);

  const handleScanEvent = useCallback((event: ScanEvent) => {
    if (event.type === 'discover.found') {
      if (SCAN_PORTS.includes(event.port)) {
        setScan(current => ({
          ...current,
          hits: addHit(current.hits, event.addr, event.port),
        }));
      }
      return;
    }
    scanRun.current = null;
    setScan(current => ({ ...current, phase: 'done' }));
  }, []);

  const onScanEvent = (event: ScanEvent) => {
    const run = scanRun.current;
    if (run === null) {
      return;
    }
    if (run.id === null) {
      run.early.push(event);
    } else if (event.scanId === run.id) {
      handleScanEvent(event);
    }
  };
  useCoreEvent('discover.found', onScanEvent);
  useCoreEvent('discover.done', onScanEvent);

  const cancelScan = useCallback(() => {
    const run = scanRun.current;
    scanRun.current = null;
    if (run?.id != null) {
      try {
        discover?.cancelScan(run.id);
      } catch {
        // The scan stops on its own; nothing else to undo here.
      }
    }
  }, [discover]);

  useEffect(() => cancelScan, [cancelScan]);

  const startScan = async () => {
    if (discover === undefined) {
      return;
    }
    const run: ScanRun = { id: null, early: [] };
    scanRun.current = run;
    setScan({ phase: 'running', hits: [] });
    try {
      const id = await discover.scanLan(SCAN_PORTS);
      if (scanRun.current !== run) {
        // Cancelled, or the page closed, before the scan id arrived.
        discover.cancelScan(id);
        return;
      }
      run.id = id;
      for (const event of run.early.splice(0)) {
        if (event.scanId === id) {
          handleScanEvent(event);
        }
      }
    } catch {
      if (scanRun.current === run) {
        scanRun.current = null;
        setScan(current => ({ ...current, phase: 'failed' }));
      }
    }
  };

  const toggleScan = () => {
    if (scan.phase === 'running') {
      cancelScan();
      setScan(current => ({ ...current, phase: 'done' }));
      return;
    }
    startScan();
  };

  const choose = (candidate: Candidate) => {
    setSaveError(null);
    setName(candidate.name);
    setPending(candidate);
  };

  /** Already in the list: tapping it connects instead of adding it again. */
  const open = (entry: ComputerEntry) => {
    setSaveError(null);
    connect(entry, { replace: true }).catch(() => undefined);
  };

  const confirm = async () => {
    if (pending === null || savingRef.current) {
      return;
    }
    savingRef.current = true;
    setAdding(true);
    const trimmed = name.trim();
    const entry: ManualComputer & { os: NativeOs } = {
      id: newComputerId(
        pending.addr,
        pending.port,
        computers.map(computer => computer.id),
      ),
      kind: 'computer',
      host: pending.addr,
      port: pending.port,
      hostId: null,
      name: trimmed === '' ? null : trimmed,
      networkId: requiredNetwork(pending.addr, networks)?.id ?? null,
      addedAtUs: Date.now() * 1000,
      os: pending.os,
      protocol: pending.protocol,
    };
    // The dialog stays up with a spinner until the session opens; the
    // session replaces this page.
    try {
      await saveComputer(entry);
    } catch {
      setSaveError(addByAddress.actions.saveFailed);
      setPending(null);
      setAdding(false);
      savingRef.current = false;
      return;
    }
    try {
      await connect(
        { kind: 'manual', key: `manual:${entry.id}`, manual: entry },
        { replace: true },
      );
    } finally {
      savingRef.current = false;
      setAdding(false);
      setPending(null);
    }
  };

  const visibleMacs = macs.filter(service => bonjourAddr(service) !== null);
  const progress = scanProgress(scan);

  return (
    <FormScroll
      contentContainerStyle={[
        styles.content,
        { paddingBottom: insets.bottom + 24 },
      ]}
      footer={
        <>
          <Dialog
            actions={[
              {
                label: native.add.save,
                loading: adding,
                onPress: () => {
                  confirm();
                },
                testID: 'add-computer-confirm',
              },
              {
                label: common.cancel,
                onPress: () => setPending(null),
                variant: 'text',
              },
            ]}
            message={pending?.addr}
            onDismiss={() => setPending(null)}
            testID="add-computer-name"
            title={addByAddress.lan.nameTitle}
            visible={pending !== null}
          >
            <TextField
              accessibilityLabel={addByAddress.lan.nameLabel}
              editable={!adding}
              label={addByAddress.lan.nameLabel}
              onChangeText={setName}
              onSubmitEditing={() => {
                confirm();
              }}
              returnKeyType="go"
              value={name}
            />
          </Dialog>
          <FallbackGuide guide={guide} onClose={closeGuide} />
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
      testID="add-computer-form"
    >
      <AppText accessibilityRole="header" variant="title" weight={700}>
        {native.add.title}
      </AppText>

      {showBonjour ? (
        <View style={styles.section} testID="add-computer-bonjour">
          <SectionHeader title={native.add.lanMacs} />
          {visibleMacs.length > 0 ? null : bonjourWaited ? (
            <View style={styles.section}>
              <AppText color="text3" variant="footnote">
                {native.add.lanEmpty}
              </AppText>
              <AppText color="text3" variant="footnote">
                {native.add.lanEmptyHint}
              </AppText>
              <Button
                align="start"
                onPress={() => {
                  Linking.openSettings().catch(() => undefined);
                }}
                testID="add-computer-open-settings"
                title={native.add.openSettings}
                variant="text"
              />
            </View>
          ) : (
            <View style={styles.searching} testID="add-computer-bonjour-searching">
              <ActivityIndicator color={tokens.accent} size="small" />
              <AppText color="text3" variant="footnote">
                {native.add.lanSearching}
              </AppText>
            </View>
          )}
          {visibleMacs.length === 0 ? null : (
            <ListGroup>
              {visibleMacs.map(service => {
                const addr = bonjourAddr(service) ?? '';
                const existing = [...service.addrs, service.host]
                  .map(value => added.get(value))
                  .find(entry => entry !== undefined);
                const isAdded = existing !== undefined;
                return (
                  <ListRow
                    key={service.name}
                    chevron
                    disabled={connecting}
                    onPress={() =>
                      existing !== undefined
                        ? open(existing)
                        : choose({
                            addr,
                            port: service.port > 0 ? service.port : VNC_PORT,
                            os: 'macos',
                            protocol: 'vnc',
                            name: service.name,
                          })
                    }
                    subtitle={addr}
                    title={service.name}
                    value={isAdded ? addByAddress.lan.added : undefined}
                  />
                );
              })}
            </ListGroup>
          )}
        </View>
      ) : null}

      {discover === undefined ? null : (
        <View style={styles.section} testID="add-computer-scan">
          <Button
            fullWidth
            onPress={toggleScan}
            testID="add-computer-scan-button"
            title={
              scan.phase === 'running'
                ? addByAddress.lan.cancelScan
                : native.add.scan
            }
            variant="secondary"
          />
          {progress === null ? null : (
            <View style={styles.searching}>
              {scan.phase === 'running' ? (
                <ActivityIndicator
                  color={tokens.accent}
                  size="small"
                  testID="add-computer-scan-spinner"
                />
              ) : null}
              <AppText
                accessibilityLiveRegion="polite"
                color="text3"
                style={styles.progress}
                variant="footnote"
              >
                {progress}
              </AppText>
            </View>
          )}
          {scan.hits.length === 0 ? null : (
            <ListGroup>
              {scan.hits.map(hit => {
                const candidate = hitCandidate(hit);
                const existing = added.get(hit.addr);
                const isAdded = existing !== undefined;
                return (
                  <ListRow
                    key={hit.addr}
                    chevron
                    disabled={connecting}
                    onPress={() =>
                      existing !== undefined
                        ? open(existing)
                        : choose(candidate)
                    }
                    subtitle={
                      candidate.os === 'macos'
                        ? addByAddress.lan.mac
                        : addByAddress.lan.windows
                    }
                    title={hit.addr}
                    value={isAdded ? addByAddress.lan.added : undefined}
                  />
                );
              })}
            </ListGroup>
          )}
        </View>
      )}

      <ListGroup>
        <ListRow
          chevron
          onPress={() =>
            navigation.navigate('AddByAddress', undefined, { pop: true })
          }
          title={native.add.manual}
        />
      </ListGroup>

      {saveError === null ? null : (
        <AppText color="bad" variant="footnote">
          {saveError}
        </AppText>
      )}

      <Button
        align="start"
        onPress={() => navigation.navigate('PairAdd', undefined, { pop: true })}
        title={native.add.installHost}
        variant="text"
      />
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
  section: {
    gap: space.sm,
  },
  searching: {
    minHeight: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
  },
  progress: {
    flex: 1,
  },
});
