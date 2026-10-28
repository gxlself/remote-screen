import {
  ActivityIndicator,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import type { AccessibilityActionEvent } from 'react-native';
import { spokenMs } from '../features/format/latency';
import { common } from '../i18n/zh/common';
import { press } from '../theme/platform';
import { tokens } from '../theme/tokens';
import AppText from './AppText';
import { PathLabel, type PathKind } from './PathMark';
import { StatusChip, type StatusChipProps } from './StatusChip';
import { Icon } from './icons/Icon';

// 同 `ripple.neutral`。
const RIPPLE = 'rgba(255, 255, 255, 0.12)';

export type DeviceCardLook = 'live' | 'sleeping' | 'offline';
export type DeviceCardStatusTone = 'muted' | 'accent' | 'warn';

export type DeviceCardProps = {
  name: string;
  look: DeviceCardLook;
  path?: PathKind;
  latencyMs?: number;
  tcp?: boolean;
  statusText?: string;
  statusTone?: DeviceCardStatusTone;
  displays?: number;
  live?: boolean;
  chip?: { label: string; tone: StatusChipProps['tone'] };
  thumbnail?: React.ReactNode;
  a11yPath?: string;
  /** This card's connection is starting: a spinner over the thumbnail. */
  busy?: boolean;
  /** Another card is connecting; taps wait for it. */
  disabled?: boolean;
  onPress: () => void;
  onLongPress?: () => void;
  testID?: string;
};

export type DeviceCardA11yInput = Partial<DeviceCardProps> &
  Pick<DeviceCardProps, 'name'>;

export function deviceCardA11yLabel({
  name,
  path,
  latencyMs,
  statusText,
  displays,
  chip,
  a11yPath,
}: DeviceCardA11yInput): string {
  const parts = [name];

  if (statusText) {
    parts.push(statusText);
  } else if (path) {
    const pathText = a11yPath ?? common.path[path];
    if (path === 'offline') {
      parts.push(pathText);
    } else if (latencyMs == null) {
      parts.push(pathText);
    } else if (a11yPath) {
      parts.push(pathText, spokenMs(latencyMs));
    } else {
      parts.push(`${pathText} ${spokenMs(latencyMs)}`);
    }
  }

  if (displays != null && displays >= 2) {
    parts.push(common.a11y.screens(displays));
  }
  if (chip) {
    parts.push(chip.label);
  }

  return parts.join('，');
}

const thumbBackgrounds: Record<DeviceCardLook, string> = {
  live: tokens.thumbBg,
  sleeping: '#10151A',
  offline: '#0F1316',
};

const statusColors: Record<DeviceCardStatusTone, string> = {
  muted: tokens.text3,
  accent: tokens.accent,
  warn: tokens.relay,
};

export function DeviceCard({
  name,
  look,
  path,
  latencyMs,
  tcp,
  statusText,
  statusTone = 'muted',
  displays,
  live = false,
  chip,
  thumbnail,
  a11yPath,
  busy = false,
  disabled = false,
  onPress,
  onLongPress,
  testID,
}: DeviceCardProps) {
  const offline = look === 'offline';
  const showDisplays = displays != null && displays >= 2;
  const android = Platform.OS === 'android';

  return (
    <Pressable
      {...(onLongPress == null
        ? null
        : {
            accessibilityActions: [
              { name: 'longpress', label: common.a11y.viewDetails },
            ],
            onAccessibilityAction: (event: AccessibilityActionEvent) => {
              if (event.nativeEvent.actionName === 'longpress') {
                onLongPress();
              }
            },
          })}
      accessibilityHint={common.a11y.deviceCardHint}
      accessibilityLabel={deviceCardA11yLabel({
        name,
        path,
        latencyMs,
        statusText,
        displays,
        chip,
        a11yPath,
      })}
      accessibilityRole="button"
      accessibilityState={
        busy || disabled ? { busy, disabled: disabled || busy } : undefined
      }
      android_ripple={android ? { color: RIPPLE, foreground: true } : undefined}
      disabled={disabled || busy}
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
        style={[styles.thumbnail, { backgroundColor: thumbBackgrounds[look] }]}
      >
        {thumbnail != null ? (
          thumbnail
        ) : look === 'sleeping' ? (
          <View style={styles.thumbnailMessage}>
            <Icon name="moon" size={22} color="text2" />
            <AppText color="text2" variant="caption">
              {common.status.sleeping}
            </AppText>
          </View>
        ) : look === 'offline' ? (
          <View style={styles.thumbnailMessage}>
            <Icon name="computerOff" size={22} color="text3" />
            <AppText color="text3" variant="caption">
              {common.status.offline}
            </AppText>
          </View>
        ) : (
          <Icon name="computer" size={22} color="text2" />
        )}

        {busy ? (
          <View
            style={styles.busy}
            testID={testID === undefined ? undefined : `${testID}-busy`}
          >
            <ActivityIndicator color={tokens.text} />
          </View>
        ) : null}

        {live ? (
          <View style={[styles.overlayBadge, styles.liveBadge]}>
            <View style={styles.liveDot} />
            <AppText style={styles.overlayText}>{common.status.live}</AppText>
          </View>
        ) : null}

        {showDisplays ? (
          <View style={[styles.overlayBadge, styles.displaysBadge]}>
            <AppText style={styles.overlayText}>
              {common.screensShort(displays)}
            </AppText>
          </View>
        ) : null}

        {chip ? (
          <View style={styles.chipBadge}>
            <StatusChip label={chip.label} tone={chip.tone} size="sm" />
          </View>
        ) : null}
      </View>

      <View style={styles.info}>
        <AppText variant="bodyStrong">{name}</AppText>
        {statusText ? (
          <AppText
            variant="caption"
            weight={500}
            style={{ color: statusColors[statusTone] }}
          >
            {statusText}
          </AppText>
        ) : path ? (
          <PathLabel kind={path} latencyMs={latencyMs} tcp={tcp} size="sm" />
        ) : null}
      </View>
    </Pressable>
  );
}

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
  androidCard: {
    overflow: 'hidden',
  },
  cardOffline: {
    backgroundColor: tokens.offlineCard,
    borderColor: tokens.inputLine,
    borderStyle: 'dashed',
  },
  thumbnail: {
    height: 88,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    overflow: 'hidden',
  },
  thumbnailMessage: {
    alignItems: 'center',
    gap: 6,
  },
  pressed: { opacity: press.opacity },
  disabled: { opacity: 0.5 },
  busy: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(10, 13, 16, 0.55)',
  },
  overlayBadge: {
    position: 'absolute',
    minHeight: 20,
    borderRadius: 10,
    paddingHorizontal: 6,
    backgroundColor: 'rgba(10, 13, 16, 0.78)',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  liveBadge: {
    top: 8,
    left: 8,
  },
  displaysBadge: {
    top: 8,
    right: 8,
  },
  overlayText: {
    fontSize: 11,
    lineHeight: 14,
  },
  liveDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: tokens.good,
  },
  chipBadge: {
    position: 'absolute',
    left: 8,
    bottom: 8,
  },
  info: {
    gap: 2,
  },
});
