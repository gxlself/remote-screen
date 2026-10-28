import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { diagnostics } from '../../i18n/zh/diagnostics';
import type { ProbeRow } from '../../features/diag/probeRows';
import { formatMs, spokenMs } from '../../features/format/latency';
import { tokens } from '../../theme/tokens';

function netName(row: ProbeRow, derpRegion?: string): string {
  switch (row.path) {
    case 'tailnetDirect':
      return diagnostics.connection.names.tailnetDirect;
    case 'tailnetDerp':
      return derpRegion === undefined || derpRegion === ''
        ? diagnostics.connection.names.tailnetDerp
        : diagnostics.connection.names.tailnetDerpRegion(derpRegion);
    case 'intranet':
      return row.transport === 'tcp'
        ? diagnostics.connection.names.intranetTcp
        : diagnostics.connection.names.intranetQuic;
    case 'lan':
      return row.transport === 'tcp'
        ? diagnostics.connection.names.lanTcp
        : diagnostics.connection.names.lanQuic;
    case 'manual':
      return row.transport === 'tcp'
        ? diagnostics.connection.names.manualTcp
        : diagnostics.connection.names.manualQuic;
  }
}

function spokenDetail(row: ProbeRow): string {
  if (row.state === 'pending') {
    return diagnostics.connection.pending;
  }
  if (row.state === 'ok') {
    return spokenMs(row.ms);
  }
  return `${diagnostics.connection.fail}，${diagnostics.connection.reason(
    row.reason,
    null,
  )}`;
}

export function ProbeRows({
  rows,
  bestKey,
  derpNames,
}: {
  rows: ProbeRow[];
  bestKey: string | null;
  derpNames?: Record<string, string>;
}) {
  return (
    <View
      accessibilityLabel={diagnostics.connection.listLabel}
      accessibilityRole="list"
      style={styles.group}
      testID="probe-rows"
    >
      {rows.map((row, index) => {
        const name = netName(row, derpNames?.[row.addr]);
        const best = row.key === bestKey;
        const subtitle =
          row.path === 'tailnetDerp'
            ? diagnostics.connection.derpViaRelay
            : row.addr;

        return (
          <View key={row.key}>
            {index === 0 ? null : <View style={styles.separator} />}
            <View
              accessibilityLabel={diagnostics.connection.a11y(
                name,
                row.addr,
                spokenDetail(row),
              )}
              accessible
              style={[styles.row, best ? styles.bestRow : null]}
              testID={`probe-row-${row.key}`}
            >
              {best ? <View style={styles.bestBar} /> : null}
              <View style={styles.left}>
                <AppText weight={500} variant="body">
                  {name}
                </AppText>
                <AppText color="text3" mono variant="monoSmall">
                  {subtitle}
                </AppText>
              </View>
              <View style={styles.right}>
                {row.state === 'pending' ? (
                  <AppText color="text3" variant="footnote">
                    {diagnostics.connection.pending}
                  </AppText>
                ) : null}
                {row.state === 'ok' ? (
                  <AppText
                    color="good"
                    mono
                    style={styles.successMs}
                    testID={`probe-ms-${row.key}`}
                    variant="mono"
                  >
                    {formatMs(row.ms)}
                  </AppText>
                ) : null}
                {row.state === 'fail' ? (
                  <View style={styles.failBlock}>
                    <AppText color="bad" weight={600} variant="footnote">
                      {diagnostics.connection.fail}
                    </AppText>
                    <AppText color="text3" variant="caption">
                      {diagnostics.connection.reason(row.reason, row.ms)}
                    </AppText>
                  </View>
                ) : null}
              </View>
            </View>
          </View>
        );
      })}
    </View>
  );
}

export { netName };

const styles = StyleSheet.create({
  group: {
    borderRadius: 16,
    backgroundColor: tokens.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    overflow: 'hidden',
  },
  separator: {
    height: 1,
    backgroundColor: tokens.cardLine,
  },
  row: {
    minHeight: 56,
    paddingHorizontal: 14,
    paddingVertical: 8,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  bestRow: {
    backgroundColor: 'rgba(93,211,154,0.06)',
  },
  bestBar: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    left: 0,
    width: 3,
    backgroundColor: tokens.good,
  },
  left: {
    flex: 1,
    minWidth: 0,
  },
  right: {
    alignItems: 'flex-end',
    justifyContent: 'center',
  },
  failBlock: {
    alignItems: 'flex-end',
    gap: 1,
  },
  successMs: {
    fontSize: 14,
    lineHeight: 20,
  },
});
