import { StyleSheet, View } from 'react-native';
import { spokenMs, formatMs } from '../features/format/latency';
import { common } from '../i18n/zh/common';
import { tokens } from '../theme/tokens';
import AppText from './AppText';

export type PathKind =
  | 'tailnetDirect'
  | 'tailnetDerp'
  | 'intranet'
  | 'lan'
  | 'manual'
  | 'offline';

export type PathMarkProps = {
  kind: PathKind;
  size?: number;
  testID?: string;
};

const HUD_PATH_LATENCY = '#A9C3FF';

export function PathMark({ kind, size, testID }: PathMarkProps) {
  const extent = size ?? (kind === 'offline' ? 9 : 7);
  const style =
    kind === 'tailnetDirect'
      ? {
          width: extent,
          height: extent,
          borderRadius: extent / 2,
          backgroundColor: tokens.good,
        }
      : kind === 'tailnetDerp'
      ? {
          width: extent,
          height: extent,
          borderRadius: 1.5,
          backgroundColor: tokens.relay,
          transform: [{ rotate: '45deg' }],
        }
      : kind === 'offline'
      ? {
          width: extent,
          height: extent,
          borderRadius: 2,
          borderWidth: 1.3,
          borderStyle: 'dashed' as const,
          borderColor: tokens.text3,
        }
      : {
          width: extent,
          height: extent,
          borderRadius: 2,
          backgroundColor: tokens.accent,
        };

  return <View accessible={false} style={style} testID={testID} />;
}

export function TcpTag() {
  return (
    <View accessible={false} style={styles.tcpTag}>
      <AppText mono color="text2" style={styles.tcpText} variant="caption">
        {common.path.tcp}
      </AppText>
    </View>
  );
}

export type PathLabelProps = {
  kind: PathKind;
  latencyMs?: number;
  tcp?: boolean;
  text?: string;
  size?: 'sm' | 'md';
  tintLatency?: boolean;
  testID?: string;
};

export function pathSpoken(kind: PathKind, latencyMs?: number): string {
  if (kind === 'offline') {
    return common.path.offline;
  }
  if (latencyMs == null) {
    return common.path[kind];
  }
  return `${common.path[kind]} ${spokenMs(latencyMs)}`;
}

function latencyColor(kind: PathKind, tintLatency: boolean): string {
  if (kind === 'tailnetDerp') {
    return tokens.relay;
  }
  if (!tintLatency) {
    return tokens.text;
  }
  switch (kind) {
    case 'tailnetDirect':
      return tokens.good;
    case 'intranet':
    case 'lan':
      return HUD_PATH_LATENCY;
    default:
      return tokens.text;
  }
}

export function PathLabel({
  kind,
  latencyMs,
  tcp = false,
  text,
  size = 'md',
  tintLatency = false,
  testID,
}: PathLabelProps) {
  const pathText = text ?? common.path[kind];
  const spoken =
    kind === 'offline'
      ? common.path.offline
      : `${pathText}${latencyMs == null ? '' : ` ${spokenMs(latencyMs)}`}`;
  const small = size === 'sm';

  return (
    <View
      accessible
      accessibilityLabel={spoken}
      style={styles.pathLabel}
      testID={testID}
    >
      <PathMark kind={kind} />
      <AppText
        variant={small ? 'caption' : 'footnote'}
        color="text2"
        weight={small ? 400 : 500}
      >
        {pathText}
      </AppText>
      {latencyMs == null ? null : (
        <AppText
          mono
          variant={small ? 'monoSmall' : 'mono'}
          style={{ color: latencyColor(kind, tintLatency) }}
        >
          {formatMs(latencyMs)}
        </AppText>
      )}
      {tcp ? <TcpTag /> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  pathLabel: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
  },
  tcpTag: {
    minHeight: 16,
    paddingHorizontal: 4,
    borderRadius: 4,
    borderWidth: 1,
    borderColor: tokens.text3,
    alignItems: 'center',
    justifyContent: 'center',
  },
  tcpText: {
    fontSize: 10,
    lineHeight: 12,
  },
});
