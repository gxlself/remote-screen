import { StyleSheet, View } from 'react-native';
import AppText from '../../../components/AppText';
import type { Computer } from '../../../native/rscore/types';
import type { HostPath } from '../../../features/directory/pathStore';
import {
  displaysCell,
  hostCell,
  lastConnectionCell,
  routeAddr,
} from '../../../features/directory/wideRows';
import type { HistoryEntry } from '../../../features/sessions/history';
import { formatMs } from '../../../features/format/latency';
import { common } from '../../../i18n/zh/common';
import { ipadHome } from '../../../i18n/zh/ipadHome';
import { tokens } from '../../../theme/tokens';

export type InfoCell = {
  label: string;
  value: string;
  sub?: string;
  testID?: string;
};

export type InfoGridProps = {
  cells: InfoCell[];
  narrow: boolean;
};

export type DetailInfo = {
  computer: Computer;
  hostPath?: HostPath;
  history: HistoryEntry[];
  nowUs: number;
};

function pathText(hostPath?: HostPath): string {
  // Measured per connection; 上次连接 says whether there was one.
  if (hostPath === undefined) {
    return ipadHome.info.pathPending;
  }
  return `${common.path[hostPath.path]}${ipadHome.separator}${formatMs(
    hostPath.latencyMs,
  )}`;
}

export function detailInfoCells({
  computer,
  history,
  hostPath,
  nowUs,
}: DetailInfo): InfoCell[] {
  const displays = displaysCell(computer);
  const host = hostCell(computer);
  const last = lastConnectionCell(history, computer.hostId, nowUs);
  const route =
    hostPath === undefined ? null : routeAddr(computer, hostPath.path);

  return [
    {
      label: ipadHome.info.path,
      value: pathText(hostPath),
      sub: route ?? undefined,
      testID: 'wide-info-path',
    },
    {
      label: ipadHome.info.displays,
      value: displays.value,
      sub: displays.sub,
      testID: 'wide-info-displays',
    },
    {
      label: ipadHome.info.host,
      value: host.value,
      sub: host.sub === '' ? undefined : host.sub,
      testID: 'wide-info-host',
    },
    {
      label: ipadHome.info.lastConnection,
      value: last === null ? ipadHome.info.neverConnected : last.value,
      sub: last === null ? undefined : last.sub,
      testID: 'wide-info-last',
    },
  ];
}

export function InfoGrid({ cells, narrow }: InfoGridProps) {
  return (
    <View style={styles.grid} testID="wide-info-grid">
      {cells.map(cell => (
        <View
          accessibilityLabel={ipadHome.a11y.infoCell(
            cell.label,
            cell.value,
            cell.sub,
          )}
          accessible
          key={cell.label}
          style={[styles.cell, narrow ? styles.cellNarrow : styles.cellWide]}
          testID={cell.testID}
        >
          <AppText color="text3" variant="caption">
            {cell.label}
          </AppText>
          <AppText style={styles.value} variant="body" weight={500}>
            {cell.value}
          </AppText>
          {cell.sub === undefined || cell.sub === '' ? null : (
            <AppText color="text3" variant="caption">
              {cell.sub}
            </AppText>
          )}
        </View>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 12,
  },
  cell: {
    borderRadius: 12,
    backgroundColor: tokens.card,
    padding: 12,
    gap: 2,
  },
  value: {
    fontSize: 14,
    lineHeight: 20,
  },
  cellWide: {
    flex: 1,
    minWidth: 0,
  },
  cellNarrow: {
    width: '46%',
    flexGrow: 1,
  },
});
