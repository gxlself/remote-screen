import { useState } from 'react';
import { StyleSheet, View } from 'react-native';
import AppText from '../../../components/AppText';
import Dialog from '../../../components/Dialog';
import { Icon } from '../../../components/icons/Icon';
import { offlineInfo } from '../../../features/errors/offlineInfo';
import { useDirectory } from '../../../features/data/queries';
import { common } from '../../../i18n/zh/common';
import { errBoards2 } from '../../../i18n/zh/errBoards2';
import { tokens } from '../../../theme/tokens';
import type { Directory } from '../../../native/rscore/types';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { MockWindow, StatusRow, summaryText } from '../parts';

const EMPTY_DIRECTORY: Directory = {
  others: [],
  generatedAt: 0,
  computers: [],
  servers: [],
};

export default function OfflineBoard({
  code,
  host,
  hostName,
  os,
  actions,
}: BoardProps) {
  const copy = errBoards2.offline;
  const { directory } = useDirectory();
  const [dialogVisible, setDialogVisible] = useState(false);
  const info =
    host === undefined
      ? undefined
      : offlineInfo(host, directory ?? EMPTY_DIRECTORY, Date.now() * 1000);
  const lastSeenRel = info?.lastSeenRel ?? copy.unknown;
  const lastSeenAt = info?.lastSeenAt ?? copy.unknown;
  const relays = info?.relays ?? [];
  const systemName =
    host?.osVersion ??
    (os === 'macos' ? 'macOS' : os === 'windows' ? 'Windows' : null);
  const illustrationSummary = summaryText([
    { label: copy.lastSeenLabel, value: lastSeenAt },
  ]);

  return (
    <>
      <ErrorFrame
        back={{ label: common.back, onPress: actions.back }}
        code={code}
        desc={`${copy.descLastSeen(lastSeenRel)}${copy.descRetry}`}
        illustrationSummary={illustrationSummary}
        illustration={
          <MockWindow title={copy.cardTitle(hostName)}>
            <View style={styles.summary}>
              <View style={styles.computerIcon}>
                <Icon color="text2" name="computerOff" size={18} />
              </View>
              <View style={styles.summaryText}>
                <AppText style={styles.rowTitle} weight={600}>
                  {copy.rowTitle(hostName)}
                </AppText>
                <AppText color="text3" style={styles.rowSubtitle}>
                  {copy.rowSubtitle(systemName, lastSeenRel)}
                </AppText>
              </View>
            </View>
            <View style={styles.rows}>
              <StatusRow label={copy.lastSeenLabel} value={lastSeenAt} />
            </View>
          </MockWindow>
        }
        kicker={copy.kicker}
        onCopyCode={actions.copyCode}
        primary={{ label: common.retry, onPress: () => actions.retry() }}
        secondary={
          // V34: no other computer on its network, nothing to offer.
          relays.length === 0
            ? undefined
            : {
                label: copy.viewRelays,
                onPress: () => setDialogVisible(true),
              }
        }
        title={copy.title(hostName)}
      />
      <Dialog
        actions={[
          ...relays.map(item => ({
            label: copy.connectRelay(item.name),
            variant: 'secondary' as const,
            onPress: () => {
              setDialogVisible(false);
              actions.openSession(item.hostId);
            },
          })),
          {
            label: common.cancel,
            onPress: () => setDialogVisible(false),
          },
        ]}
        onDismiss={() => setDialogVisible(false)}
        testID="relay-dialog"
        title={copy.relayDialogTitle}
        visible={dialogVisible}
      />
    </>
  );
}

const styles = StyleSheet.create({
  summary: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    borderRadius: 12,
    backgroundColor: tokens.raised,
    padding: 10,
  },
  computerIcon: {
    width: 38,
    height: 30,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 6,
    borderWidth: 1,
    borderColor: tokens.inputLine,
    backgroundColor: tokens.card,
  },
  summaryText: { flex: 1, gap: 2 },
  rowTitle: { fontSize: 12.5, lineHeight: 17 },
  rowSubtitle: { fontSize: 10.5, lineHeight: 14 },
  rows: { gap: 2 },
});
