import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Switch } from '../../components/Switch';
import { Toast, useToast } from '../../components/Toast';
import {
  advice,
  applyEmbeddedPaths,
  derpNamesFor,
  type ProbeRow,
} from '../../features/diag/probeRows';
import { MIN_HOST_VERSION } from '../../features/diagnostics/hostVersion';
import { useNetwork } from '../../features/events/EventsProvider';
import { formatMs } from '../../features/format/latency';
import { formatHm } from '../../features/format/time';
import { useDirectory } from '../../features/data/queries';
import { judgeNetwork } from '../../features/network/judge';
import { useSetting } from '../../features/settings/store';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { embeddedAddr } from '../../features/tailnet/peers';
import { useEmbeddedTailnet } from '../../features/tailnet/useEmbeddedTailnet';
import { diagnostics } from '../../i18n/zh/diagnostics';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import type { Computer } from '../../native/rscore/types';
import { hit, space, tokens } from '../../theme/tokens';
import { HostPicker } from './HostPicker';
import { netName, ProbeRows as ProbeRowsView } from './ProbeRows';
import { useProbeRun } from './useProbeRun';

function defaultComputer(
  computers: Computer[],
  hostId: string | undefined,
): Computer | null {
  if (hostId !== undefined) {
    const target = computers.find(computer => computer.hostId === hostId);
    if (target !== undefined) {
      return target;
    }
  }
  return computers.find(computer => computer.online) ?? computers[0] ?? null;
}

export default function DiagnosticsScreen({
  navigation,
  route,
}: RootScreenProps<'Diagnostics'>) {
  const insets = useSafeAreaInsets();
  const core = useCore();
  const platform = usePlatform();
  const directory = useDirectory();
  const computers = useMemo(
    () => directory.directory?.computers ?? [],
    [directory.directory],
  );
  const params = route.params;
  const [pickedHostId, setPickedHostId] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [mask, setMask] = useState(true);
  const [exporting, setExporting] = useState(false);
  const [networks] = useSetting('networks');
  const networkStatus = useNetwork(state => state.status);
  const toast = useToast();
  const fallback = defaultComputer(computers, params?.hostId);
  const selected =
    computers.find(computer => computer.hostId === pickedHostId) ?? fallback;
  const selectedHostId = selected?.hostId ?? null;
  const run = useProbeRun(selected);
  const embedded = useEmbeddedTailnet({ pollMs: 10_000 });
  const tailnetEnabled = useTailnetEnabled();
  const embeddedActive =
    tailnetEnabled && embedded.route?.active === 'embedded';
  // With Tailscale off its (direct / relay) rows are left out.
  const rows = useMemo(
    () =>
      embeddedActive
        ? applyEmbeddedPaths(run.rows, embedded.status)
        : tailnetEnabled
        ? run.rows
        : run.rows.filter(
            row => row.path !== 'tailnetDirect' && row.path !== 'tailnetDerp',
          ),
    [embedded.status, embeddedActive, run.rows, tailnetEnabled],
  );
  const derpNames = useMemo(
    () => (embeddedActive ? derpNamesFor(run.rows, embedded.status) : {}),
    [embedded.status, embeddedActive, run.rows],
  );
  const verdict = judgeNetwork(networkStatus, networks);
  const done = !run.running && run.finishedAtUs !== null;
  const decision = advice(rows, done);
  const previouslyDone = useRef(done);
  const bestKey = decision.kind === 'best' ? decision.row.key : null;

  useEffect(() => {
    if (Platform.OS === 'ios' && done && !previouslyDone.current) {
      AccessibilityInfo.announceForAccessibility(
        adviceText(decision, rows, derpNames),
      );
    }
    previouslyDone.current = done;
  }, [decision, derpNames, done, rows]);

  const inApp = embeddedAddr(embedded.route);
  const localNetwork = [
    verdict.companyVpn === null
      ? null
      : diagnostics.localNetwork.companyVpn(verdict.companyVpn.addr),
    !tailnetEnabled
      ? null
      : inApp !== null
      ? diagnostics.localNetwork.embedded(inApp)
      : verdict.tailnetAddr === null
      ? null
      : diagnostics.localNetwork.tailnet(verdict.tailnetAddr),
  ]
    .filter((part): part is string => part !== null)
    .join(' · ');

  const exportText = () => {
    if (selected === null) {
      return '';
    }
    const time = run.finishedAtUs === null ? '—' : formatHm(run.finishedAtUs);
    const lines = [
      diagnostics.exportText.title(selected.name, selected.node, time),
      ...rows.map(row => {
        const name = netName(row, derpNames[row.addr]);
        if (row.state === 'ok') {
          return diagnostics.exportText.okLine(
            name,
            row.addr,
            formatMs(row.ms),
          );
        }
        return diagnostics.exportText.failLine(
          name,
          row.addr,
          diagnostics.connection.reasonExport(row.reason, row.ms),
        );
      }),
      adviceText(decision, rows, derpNames),
      diagnostics.exportText.local(
        localNetwork === '' ? diagnostics.localNetwork.unknown : localNetwork,
      ),
    ];
    return lines.join('\n');
  };

  const onCopy = () => {
    platform
      .copyText(exportText())
      .then(() => toast.show(diagnostics.actions.copied, { tone: 'good' }))
      .catch(() => undefined);
  };

  const onExport = () => {
    if (exporting) {
      return;
    }
    setExporting(true);
    core.logs
      .export({ mask })
      .then(path =>
        platform.share({
          url: `file://${path}`,
          title: diagnostics.logs.exportTitle,
        }),
      )
      .catch(error => {
        toast.show(
          diagnostics.logs.exportFailed(
            error instanceof Error ? error.message : String(error),
          ),
          { tone: 'bad' },
        );
      })
      .finally(() => setExporting(false));
  };

  return (
    <View
      style={[styles.screen, { paddingTop: insets.top }]}
      testID="diagnostics-screen"
    >
      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: space.section + insets.bottom },
        ]}
        showsVerticalScrollIndicator={false}
        testID="diagnostics"
      >
        <Pressable
          accessibilityLabel={diagnostics.back}
          accessibilityRole="button"
          onPress={() => navigation.goBack()}
          style={[styles.back, { minHeight: hit.min }]}
        >
          <Icon color="accent" name="chevronLeft" size={18} />
          <AppText color="accent" variant="body">
            {diagnostics.backDestination}
          </AppText>
        </Pressable>

        <AppText accessibilityRole="header" variant="title" weight={700}>
          {diagnostics.title}
        </AppText>

        <View style={styles.section}>
          <SectionHeader
            right={
              run.running
                ? diagnostics.connection.runningRight
                : run.finishedAtUs === null
                ? undefined
                : diagnostics.connection.finishedRight(
                    formatHm(run.finishedAtUs),
                    rows.length,
                  )
            }
            title={diagnostics.connection.title}
          />

          {selected === null ? (
            <EmptyState
              icon="computerOff"
              message={diagnostics.connection.emptyMessage}
              title={diagnostics.connection.empty}
            />
          ) : (
            <>
              <ListGroup>
                <ListRow
                  right={
                    <Button
                      accessibilityHint={diagnostics.a11y.changeHint(
                        selected.name,
                      )}
                      onPress={() => setPickerOpen(true)}
                      title={diagnostics.connection.change}
                      variant="text"
                    />
                  }
                  subtitle={diagnostics.connection.subtitle(
                    selected.node,
                    selected.hostVersion,
                  )}
                  title={selected.name}
                />
              </ListGroup>

              <ProbeRowsView
                bestKey={bestKey}
                derpNames={derpNames}
                rows={rows}
              />

              <AppText
                accessibilityLiveRegion="polite"
                color="text2"
                style={styles.advice}
                variant="footnote"
              >
                {adviceText(decision, rows, derpNames)}
              </AppText>

              <View style={styles.actions} testID="diagnostics-actions">
                <Button
                  onPress={() => {
                    if (run.running) {
                      run.stop();
                    } else {
                      run.start();
                    }
                  }}
                  title={
                    run.running
                      ? diagnostics.actions.stop
                      : diagnostics.actions.retest
                  }
                  variant="secondary"
                />
                <Button
                  disabled={!done}
                  onPress={onCopy}
                  title={diagnostics.actions.copy}
                  variant="secondary"
                />
              </View>
            </>
          )}

          <AppText color="text3" style={styles.local} variant="caption">
            {`${diagnostics.localNetwork.label} · ${
              localNetwork === ''
                ? diagnostics.localNetwork.unknown
                : localNetwork
            }`}
          </AppText>
        </View>

        <View style={styles.section}>
          <SectionHeader title={diagnostics.logs.title} />
          <ListGroup>
            <ListRow
              right={
                <Button
                  loading={exporting}
                  onPress={onExport}
                  title={diagnostics.logs.export}
                  variant="pill"
                />
              }
              subtitle={diagnostics.logs.localLogsSubtitle}
              title={diagnostics.logs.localLogs}
            />
            <ListRow
              right={
                <Switch
                  accessibilityLabel={diagnostics.logs.maskA11y}
                  onValueChange={setMask}
                  testID="diagnostics-mask"
                  value={mask}
                />
              }
              subtitle={diagnostics.logs.maskSubtitle}
              title={diagnostics.logs.maskTitle}
            />
          </ListGroup>
        </View>

        <AppText center color="text3" style={styles.footer} variant="caption">
          {diagnostics.footer(
            platform.deviceInfo().appVersion,
            MIN_HOST_VERSION,
          )}
        </AppText>
      </ScrollView>

      <HostPicker
        computers={computers}
        onClose={() => setPickerOpen(false)}
        onSelect={computer => {
          setPickedHostId(computer.hostId);
          setPickerOpen(false);
        }}
        selectedHostId={selectedHostId}
        visible={pickerOpen}
      />
      <Toast onHide={toast.hide} position="bottom" toast={toast.toast} />
    </View>
  );
}

function adviceText(
  decision: ReturnType<typeof advice>,
  rows: ProbeRow[],
  derpNames: Record<string, string>,
): string {
  if (decision.kind === 'running') {
    return diagnostics.advice.running;
  }
  if (decision.kind === 'allFailed') {
    return diagnostics.advice.allFailed;
  }
  const row = rows.find(item => item.key === decision.row.key) ?? decision.row;
  let text = diagnostics.advice.best(
    netName(row, derpNames[row.addr]),
    formatMs(row.ms),
  );
  if (decision.tcpMode) {
    text += diagnostics.advice.tcpMode;
  }
  return text;
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    padding: space.gutter,
    gap: space.lg,
  },
  back: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  section: {
    gap: space.sm,
  },
  local: {
    marginTop: space.xs,
  },
  advice: {
    lineHeight: 19,
  },
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: space.sm,
  },
  footer: {
    marginTop: space.sm,
  },
});
