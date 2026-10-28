import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  PixelRatio,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import AppText from '../../components/AppText';
import { HudPill } from '../../components/HudPill';
import { Icon } from '../../components/icons/Icon';
import {
  formatBitrate,
  formatMs,
  spokenMs,
} from '../../features/format/latency';
import { useVideoStats } from '../../features/session/videoStore';
import { useHudExpansion } from '../../features/session/useHudExpansion';
import { useScreenReader } from '../../features/session/useScreenReader';
import RecordingChip from '../../features/recording/RecordingChip';
import { useSetting } from '../../features/settings/store';
import { common } from '../../i18n/zh/common';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import type { SessionRecord } from '../../features/session/store';
import type { RemoteVideoStats } from '../../native/session/RemoteSessionView';
import { hit, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';
import { useBelowTopStack } from './topStack';

const TIP_MS = 3000;
const VIDEO_STATS_STALE_MS = 3000;
const HIDDEN_FROM_A11Y = {
  accessibilityElementsHidden: true,
  importantForAccessibility: 'no-hide-descendants',
} as const;
const RELAY_BORDER = 'rgba(244,178,78,0.4)';
const STATS_TEXT = '#D5DCE0';
const LARGE_STATS_STYLE = { width: undefined, minWidth: 168 } as const;
const LARGE_TIP_STYLE = { height: undefined, minHeight: 32 } as const;

type ClipboardEntry = SessionRecord['clipboard'][number];

function clipboardKey(entry: ClipboardEntry | undefined): string {
  if (entry === undefined) {
    return '';
  }
  return `${entry.direction}\u0000${entry.at}\u0000${entry.text}`;
}

function decodeMs(ms: number | null, spoken = false): string {
  if (ms === null || !Number.isFinite(ms)) {
    return sessionToolbar.stats.unavailable;
  }
  if (ms < 1) {
    return spoken ? `<1 ${common.units.msSpoken}` : `<1 ${common.units.ms}`;
  }
  const value = String(Math.round(ms * 10) / 10);
  return `${value} ${spoken ? common.units.msSpoken : common.units.ms}`;
}

function useFreshVideoStats(sessionId: SlotProps['ctx']['sessionId']) {
  const video = useVideoStats(sessionId);
  const received = useRef<{
    sessionId: SlotProps['ctx']['sessionId'];
    video: RemoteVideoStats | null;
    atMs: number;
  } | null>(null);
  const [, setTick] = useState(0);

  if (
    received.current === null ||
    received.current.sessionId !== sessionId ||
    received.current.video !== video
  ) {
    received.current = { sessionId, video, atMs: Date.now() };
  }

  useEffect(() => {
    if (video === null || received.current === null) {
      return undefined;
    }
    const ageMs = Date.now() - received.current.atMs;
    const timer = setTimeout(
      () => setTick(tick => tick + 1),
      Math.max(0, VIDEO_STATS_STALE_MS - ageMs) + 1,
    );
    return () => clearTimeout(timer);
  }, [sessionId, video]);

  if (video === null || received.current === null) {
    return null;
  }
  return Date.now() - received.current.atMs > VIDEO_STATS_STALE_MS
    ? null
    : video;
}

function StatsOverlay({
  ctx,
  video,
  largeText,
}: SlotProps & { video: RemoteVideoStats | null; largeText: boolean }) {
  const stats = ctx.rec?.stats ?? null;
  const unavailable = sessionToolbar.stats.unavailable;
  const latency = stats === null ? unavailable : formatMs(stats.rttMs);
  const nativeFps = video === null ? null : Math.round(video.fps);
  const fps =
    nativeFps !== null
      ? String(nativeFps)
      : stats === null
      ? unavailable
      : String(Math.round(stats.fps));
  const bitrate =
    stats === null ? unavailable : formatBitrate(stats.bitrateKbps);
  const decode =
    video === null
      ? decodeMs(stats?.decodeMs ?? null)
      : sessionToolbar.stats.decodePair(
          Math.round(video.decodeP50Ms),
          Math.round(video.decodeP95Ms),
        );
  const spokenFps =
    nativeFps !== null
      ? `${nativeFps} ${common.units.fpsSpoken}`
      : stats === null
      ? unavailable
      : `${Math.round(stats.fps)} ${common.units.fpsSpoken}`;
  const accessibilityLabel =
    video === null
      ? sessionToolbar.stats.a11y(
          stats === null ? unavailable : spokenMs(stats.rttMs),
          spokenFps,
          bitrate,
          decodeMs(stats?.decodeMs ?? null, true),
        )
      : sessionToolbar.stats.a11yPair(
          stats === null ? unavailable : spokenMs(stats.rttMs),
          spokenFps,
          bitrate,
          Math.round(video.decodeP50Ms),
          Math.round(video.decodeP95Ms),
        );

  return (
    <Pressable
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      accessible
      onPress={() => ctx.openSheet('quality')}
      style={[styles.stats, largeText ? LARGE_STATS_STYLE : null]}
      testID="session-stats"
    >
      <AppText mono style={styles.statsText} testID="stats-rtt">
        {sessionToolbar.stats.rtt(latency)}
      </AppText>
      <AppText mono style={styles.statsText} testID="stats-fps">
        {sessionToolbar.stats.fps(fps)}
      </AppText>
      <AppText mono style={styles.statsText} testID="stats-bitrate">
        {sessionToolbar.stats.bitrate(bitrate)}
      </AppText>
      <AppText mono style={styles.statsText} testID="stats-decode">
        {sessionToolbar.stats.decode(decode)}
      </AppText>
    </Pressable>
  );
}

export function SessionHud({ ctx }: SlotProps) {
  const [showHud] = useSetting('session.hud');
  const expansion = useHudExpansion(ctx.hud);
  const screenReader = useScreenReader();
  const tipTop = useBelowTopStack();
  const video = useFreshVideoStats(ctx.sessionId);
  const largeText = PixelRatio.getFontScale() > 2;
  const pillPadding = Math.max(0, (hit.min - 32) / 2);
  const [autoSync] = useSetting('clipboard.autoSync');
  const [tip, setTip] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const head = ctx.rec?.clipboard[0];
  const previousClipboard = useRef(clipboardKey(head));
  const previousMode = useRef(ctx.mode);
  const previousTip = useRef<string | null>(null);

  useEffect(() => {
    const appeared = tip !== null && tip !== previousTip.current;
    previousTip.current = tip;
    if (appeared && screenReader) {
      AccessibilityInfo.announceForAccessibility(tip);
    }
  }, [screenReader, tip]);

  useEffect(() => {
    const nextClipboard = clipboardKey(head);
    const clipboardChanged =
      nextClipboard !== previousClipboard.current &&
      head?.direction === 'fromHost';
    const modeChanged = ctx.mode !== previousMode.current;
    previousClipboard.current = nextClipboard;
    previousMode.current = ctx.mode;

    if (!clipboardChanged && !modeChanged) {
      return;
    }
    if (timer.current !== null) {
      clearTimeout(timer.current);
    }
    const count = head === undefined ? 0 : Array.from(head.text).length;
    // 自动同步关着时剪贴板没有写进本机，不能说「已同步」。
    setTip(
      clipboardChanged && head !== undefined
        ? autoSync
          ? sessionToolbar.tips.clipboard(count)
          : sessionToolbar.tips.clipboardManual(count)
        : sessionToolbar.tips.mode[ctx.mode],
    );
    timer.current = setTimeout(() => {
      timer.current = null;
      setTip(null);
    }, TIP_MS);
    // 只在剪贴板或操作方式变化时出提示；开关变化本身不提示。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.mode, head]);

  useEffect(
    () => () => {
      if (timer.current !== null) {
        clearTimeout(timer.current);
      }
    },
    [],
  );

  const pill = (
    <View
      {...(screenReader ? HIDDEN_FROM_A11Y : null)}
      style={{ padding: pillPadding }}
    >
      <View
        style={[
          styles.pillFrame,
          ctx.hud.path === 'tailnetDerp' ? styles.relayFrame : null,
        ]}
      >
        <HudPill
          accessibilityLabel={ctx.hud.a11y}
          alertText={ctx.hud.alertText ?? undefined}
          fps={
            video === null ? ctx.hud.fps ?? undefined : Math.round(video.fps)
          }
          latencyMs={ctx.hud.latencyMs ?? undefined}
          mode={expansion}
          onPress={() => ctx.openSheet('quality')}
          path={ctx.hud.path}
          pathText={ctx.hud.pathText}
          tcp={ctx.hud.tcp}
          testID="session-hud-pill"
        />
      </View>
    </View>
  );

  return (
    <>
      <View
        pointerEvents="box-none"
        style={[
          styles.hudLayer,
          !showHud
            ? { top: 12 - pillPadding, left: hit.landscapeSafe - pillPadding }
            : null,
        ]}
        testID="session-hud-row"
      >
        {showHud ? (
          <View {...(screenReader ? HIDDEN_FROM_A11Y : null)}>
            <StatsOverlay ctx={ctx} video={video} largeText={largeText} />
          </View>
        ) : (
          pill
        )}
        {/* 录制芯片并排在 HUD 右边，不再压住胶囊，也避开横屏的灵动岛一侧。 */}
        <View style={showHud ? null : { marginTop: pillPadding }}>
          <RecordingChip ctx={ctx} />
        </View>
      </View>
      <View
        {...(screenReader ? HIDDEN_FROM_A11Y : null)}
        pointerEvents="none"
        // 提示放在顶部提示栏下面，不压住 HUD 和共享提示（R37）。
        style={[styles.tipLayer, { top: tipTop }]}
      >
        {tip === null ? null : (
          <View
            accessibilityLabel={tip}
            accessibilityLiveRegion="polite"
            accessible
            style={[styles.tip, largeText ? LARGE_TIP_STYLE : null]}
            testID="session-tip"
          >
            <Icon color="good" name="check" size={13} />
            <AppText color="text" style={styles.tipText} variant="caption">
              {tip}
            </AppText>
          </View>
        )}
      </View>
    </>
  );
}

const styles = StyleSheet.create({
  hudLayer: {
    position: 'absolute',
    left: hit.landscapeSafe,
    top: 12,
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 8,
  },
  pillFrame: {
    borderRadius: 16,
  },
  relayFrame: {
    borderWidth: 1,
    borderColor: RELAY_BORDER,
  },
  stats: {
    width: 168,
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 12,
    backgroundColor: tokens.hud,
    gap: 0,
  },
  statsText: {
    color: STATS_TEXT,
    fontSize: 11,
    lineHeight: 17,
  },
  tipLayer: {
    position: 'absolute',
    left: 0,
    right: 0,
    alignItems: 'center',
  },
  tip: {
    height: 32,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 7,
    paddingHorizontal: 12,
    borderRadius: 16,
    backgroundColor: tokens.hud,
  },
  tipText: {
    fontSize: 12,
  },
});

export default SessionHud;
