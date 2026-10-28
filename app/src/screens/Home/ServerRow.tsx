import { Platform, Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { StatusChip } from '../../components/StatusChip';
import { Icon } from '../../components/icons/Icon';
import { common } from '../../i18n/zh/common';
import { home } from '../../i18n/zh/home';
import { press } from '../../theme/platform';
import { tokens } from '../../theme/tokens';
import type { ServerItem } from './useHomeModel';

export type ServerRowProps = {
  server: ServerItem;
  onPress: () => void;
};

function statusText(online: boolean): string {
  return online ? common.status.online : common.status.offline;
}

function rowLabel(server: ServerItem): string {
  if (server.suggestion !== undefined) {
    return home.servers.suggestionLabel(
      server.name,
      server.suggestion.host,
      statusText(server.suggestion.online),
    );
  }
  return server.needsTailnet
    ? `${server.name}，${home.servers.needsTailnet(server.subtitle)}`
    : `${server.name}，${server.subtitle}`;
}

export function ServerRow({ server, onPress }: ServerRowProps) {
  return (
    <Pressable
      accessibilityLabel={rowLabel(server)}
      accessibilityRole="button"
      // 同 ripple.neutral
      android_ripple={
        Platform.OS === 'android'
          ? { color: 'rgba(255, 255, 255, 0.12)' }
          : undefined
      }
      onPress={onPress}
      style={({ pressed }) => [
        styles.row,
        pressed && Platform.OS !== 'android' ? styles.pressed : null,
      ]}
      testID={`server-${server.serverId}`}
    >
      <View style={styles.iconWrap}>
        <Icon color="text2" name="terminal" size={18} />
      </View>
      <View style={styles.copy}>
        <AppText variant="bodyStrong" weight={500}>
          {server.name}
        </AppText>
        <AppText color="text3" variant="caption">
          {server.needsTailnet
            ? home.servers.needsTailnet(server.subtitle)
            : server.subtitle}
        </AppText>
      </View>
      {server.suggestion === undefined ? (
        <Icon color="text3" name="chevronRight" size={16} />
      ) : (
        <>
          <StatusChip
            label={statusText(server.suggestion.online)}
            size="sm"
            tone={server.suggestion.online ? 'good' : 'offline'}
          />
          <Icon color="text3" name="plus" size={16} />
        </>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: {
    minHeight: 60,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingLeft: 14,
    paddingRight: 14,
    paddingVertical: 8,
  },
  pressed: {
    backgroundColor: press.highlight,
  },
  iconWrap: {
    width: 36,
    height: 36,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  copy: {
    flex: 1,
  },
});
