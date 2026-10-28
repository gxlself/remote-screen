import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { PathMark } from '../../components/PathMark';
import { StatusChip } from '../../components/StatusChip';
import { formatMs } from '../../features/format/latency';
import {
  routeAccessibilityLabel,
  type RouteRowModel,
} from '../../features/directory/routes';
import { deviceRoutes } from '../../i18n/zh/deviceRoutes';
import { tokens } from '../../theme/tokens';

export function RouteRow({ row }: { row: RouteRowModel }) {
  const label = routeAccessibilityLabel(row);

  return (
    <View
      accessible
      accessibilityLabel={label}
      style={[styles.row, row.inUse ? styles.activeRow : null]}
      testID={`route-${row.key}`}
    >
      {row.inUse ? (
        <View style={styles.activeBar} testID="route-active-bar" />
      ) : null}
      <View style={styles.mark}>
        <PathMark kind={row.mark} />
      </View>
      <View style={styles.copy}>
        <View style={styles.titleLine}>
          <AppText variant="body" weight={500}>
            {row.title}
          </AppText>
          {row.inUse ? (
            <StatusChip label={deviceRoutes.current} size="sm" tone="accent" />
          ) : null}
        </View>
        <AppText color="text2" mono variant="mono">
          {row.addrText}
        </AppText>
        <AppText color="text3" variant="caption">
          {row.subtitle}
        </AppText>
      </View>
      <AppText
        color={row.latencyMs === null ? 'text3' : 'text2'}
        mono
        style={styles.latency}
        variant="mono"
      >
        {formatMs(row.latencyMs)}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  row: {
    minHeight: 76,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 14,
    paddingVertical: 8,
    gap: 10,
  },
  activeRow: {
    backgroundColor: 'rgba(125,168,255,0.07)',
  },
  activeBar: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    left: 0,
    width: 3,
    backgroundColor: tokens.accent,
  },
  mark: {
    width: 18,
    alignItems: 'center',
  },
  copy: {
    flex: 1,
    gap: 1,
  },
  titleLine: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  latency: {
    minWidth: 42,
    textAlign: 'right',
  },
});

export default RouteRow;
