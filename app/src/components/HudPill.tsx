import { PixelRatio, Platform, Pressable, StyleSheet, View } from 'react-native';
import { formatFps, formatMs } from '../features/format/latency';
import { common } from '../i18n/zh/common';
import { hit, tokens } from '../theme/tokens';
import AppText from './AppText';
import { PathMark, TcpTag, type PathKind } from './PathMark';

export type HudPillMode = 'expanded' | 'collapsed' | 'alert';

export type HudPillProps = {
  mode: HudPillMode;
  path: PathKind;
  pathText?: string;
  latencyMs?: number;
  fps?: number;
  tcp?: boolean;
  alertText?: string;
  onPress?: () => void;
  accessibilityLabel: string;
  testID?: string;
};

const HUD_PATH_LATENCY = '#A9C3FF';
const HUD_FPS = '#D5DCE0';
const ALERT_BACKGROUND = 'rgba(244,178,78,0.92)';
// 同 `ripple.neutral`。
const RIPPLE = 'rgba(255, 255, 255, 0.12)';

function latencyColor(path: PathKind): string {
  if (path === 'tailnetDirect') {
    return tokens.good;
  }
  if (path === 'tailnetDerp') {
    return tokens.relay;
  }
  if (path === 'intranet' || path === 'lan') {
    return HUD_PATH_LATENCY;
  }
  return tokens.text2;
}

export function HudPill({
  mode,
  path,
  pathText,
  latencyMs,
  fps,
  tcp = false,
  alertText,
  onPress,
  accessibilityLabel,
  testID,
}: HudPillProps) {
  const collapsed = mode === 'collapsed';
  const alert = mode === 'alert';
  const resolvedPathText = pathText ?? common.path[path];
  const android = Platform.OS === 'android';
  const largeText = PixelRatio.getFontScale() > 2;
  const slop = Math.max(0, (hit.min - 32) / 2);

  return (
    <Pressable
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      android_ripple={
        android ? { color: RIPPLE, foreground: true } : undefined
      }
      hitSlop={
        collapsed
          ? { top: slop, bottom: slop, left: slop, right: slop }
          : { top: slop, bottom: slop }
      }
      onPress={onPress}
      style={({ pressed }) => [
        styles.pill,
        collapsed ? styles.collapsed : styles.expanded,
        alert ? styles.alert : styles.defaultBackground,
        !collapsed && largeText ? { height: undefined, minHeight: 32 } : null,
        android ? styles.androidPill : null,
        pressed && !android ? styles.pressed : null,
      ]}
      testID={testID}
    >
      {collapsed ? (
        <PathMark kind={path} />
      ) : alert ? (
        <>
          <View style={styles.alertMark} />
          {alertText == null ? null : (
            <AppText
              color="onRelay"
              style={styles.alertText}
              variant="caption"
              weight={600}
            >
              {alertText}
            </AppText>
          )}
        </>
      ) : (
        <>
          <PathMark kind={path} />
          <AppText color="text2" variant="caption" weight={500}>
            {resolvedPathText}
          </AppText>
          {latencyMs == null ? null : (
            <AppText
              mono
              style={{ color: latencyColor(path) }}
              variant="monoSmall"
            >
              {formatMs(latencyMs)}
            </AppText>
          )}
          {latencyMs != null && fps != null ? (
            <AppText color="muted" variant="caption">
              ·
            </AppText>
          ) : null}
          {fps == null ? null : (
            <AppText mono style={{ color: HUD_FPS }} variant="monoSmall">
              {formatFps(fps)}
            </AppText>
          )}
          {tcp ? <TcpTag /> : null}
        </>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    borderWidth: 1,
    borderColor: tokens.hudLine,
  },
  expanded: {
    height: 32,
    borderRadius: 16,
    gap: 7,
    paddingHorizontal: 12,
  },
  collapsed: {
    width: 32,
    height: 32,
    borderRadius: 16,
    justifyContent: 'center',
  },
  defaultBackground: {
    backgroundColor: tokens.hud,
  },
  alert: {
    height: 32,
    borderRadius: 16,
    gap: 7,
    paddingHorizontal: 12,
    backgroundColor: ALERT_BACKGROUND,
    borderColor: 'transparent',
  },
  alertMark: {
    width: 7,
    height: 7,
    borderRadius: 1.5,
    backgroundColor: tokens.onRelay,
    transform: [{ rotate: '45deg' }],
  },
  alertText: {
    fontSize: 12,
    lineHeight: 16,
  },
  pressed: {
    opacity: 0.8,
  },
  androidPill: {
    overflow: 'hidden',
  },
});
