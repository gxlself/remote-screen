import { ActivityIndicator, Pressable, StyleSheet, View } from 'react-native';
import type { AccessibilityActionEvent } from 'react-native';
import AppText from '../../components/AppText';
import { StatusChip } from '../../components/StatusChip';
import { Icon } from '../../components/icons/Icon';
import type { NativeCardModel } from '../../features/directory/cardModel';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import { isAndroid, press, ripple } from '../../theme/platform';
import { tokens } from '../../theme/tokens';

export type NativeCardProps = {
  card: NativeCardModel;
  onPress(): void;
  onLongPress(): void;
  /** This card's connection is starting. */
  busy?: boolean;
  /** Another card is connecting. */
  disabled?: boolean;
  testID?: string;
};

export function nativeCardA11yLabel(card: NativeCardModel): string {
  return native.a11y.card([
    card.name,
    card.subtitle,
    card.reach === 'unknown' ? native.a11y.reachUnknown : card.reachText,
    card.badge,
  ]);
}

/** Home card for a computer without Host: system-sharing badge, no thumbnail. */
export function NativeCard({
  card,
  onPress,
  onLongPress,
  busy = false,
  disabled = false,
  testID,
}: NativeCardProps) {
  const offline = card.look === 'offline';
  const android = isAndroid();

  return (
    <Pressable
      accessibilityActions={[
        { name: 'longpress', label: common.a11y.viewDetails },
      ]}
      accessibilityHint={common.a11y.deviceCardHint}
      accessibilityLabel={nativeCardA11yLabel(card)}
      accessibilityRole="button"
      accessibilityState={
        busy || disabled ? { busy, disabled: disabled || busy } : undefined
      }
      android_ripple={
        android ? { color: ripple.neutral, foreground: true } : undefined
      }
      disabled={disabled || busy}
      onAccessibilityAction={(event: AccessibilityActionEvent) => {
        if (event.nativeEvent.actionName === 'longpress') {
          onLongPress();
        }
      }}
      onLongPress={onLongPress}
      onPress={onPress}
      style={({ pressed }) => [
        styles.card,
        offline ? styles.cardOffline : styles.cardOnline,
        android ? styles.androidCard : null,
        pressed && !android ? styles.pressed : null,
        disabled && !busy ? styles.disabled : null,
      ]}
      testID={testID}
    >
      <View
        style={[
          styles.thumbnail,
          offline ? styles.thumbnailOffline : styles.thumbnailLive,
        ]}
      >
        <Icon
          color={offline ? 'text3' : 'text2'}
          name={offline ? 'computerOff' : 'computer'}
          size={22}
        />
        <View style={styles.reach}>
          {card.reach === 'online' ? <View style={styles.dot} /> : null}
          <AppText
            color={card.reach === 'online' ? 'good' : 'text3'}
            variant="caption"
          >
            {card.reachText}
          </AppText>
        </View>
        <View style={styles.badge}>
          <StatusChip
            label={card.badge}
            size="sm"
            testID={`native-badge-${card.key}`}
            tone="accent"
          />
        </View>
        {busy ? (
          <View
            style={styles.busy}
            testID={testID === undefined ? undefined : `${testID}-busy`}
          >
            <ActivityIndicator color={tokens.text} />
          </View>
        ) : null}
      </View>

      <View style={styles.info}>
        <AppText variant="bodyStrong">{card.name}</AppText>
        <AppText color="text3" variant="caption" weight={500}>
          {card.subtitle}
        </AppText>
      </View>
    </Pressable>
  );
}

// Same frame as DeviceCard so both kinds line up in the grid.
const styles = StyleSheet.create({
  card: {
    paddingTop: 8,
    paddingHorizontal: 8,
    paddingBottom: 12,
    borderRadius: 16,
    borderWidth: 1,
    gap: 10,
  },
  cardOnline: {
    backgroundColor: tokens.card,
    borderColor: tokens.cardLine,
  },
  cardOffline: {
    backgroundColor: tokens.offlineCard,
    borderColor: tokens.inputLine,
    borderStyle: 'dashed',
  },
  androidCard: {
    overflow: 'hidden',
  },
  pressed: { opacity: press.opacity },
  disabled: { opacity: 0.5 },
  busy: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(10, 13, 16, 0.55)',
  },
  thumbnail: {
    height: 88,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    overflow: 'hidden',
  },
  thumbnailLive: {
    backgroundColor: tokens.thumbBg,
  },
  thumbnailOffline: {
    backgroundColor: tokens.offlineCard,
  },
  reach: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: tokens.good,
  },
  badge: {
    position: 'absolute',
    top: 8,
    right: 8,
  },
  info: {
    gap: 2,
  },
});
