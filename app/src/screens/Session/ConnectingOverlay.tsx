import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Animated,
  AccessibilityInfo,
  PixelRatio,
  ScrollView,
  Easing,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { PathMark } from '../../components/PathMark';
import { common } from '../../i18n/zh/common';
import { sessionConnect } from '../../i18n/zh/sessionConnect';
import {
  connectSteps,
  FIRST_FRAME_SLOW_MS,
} from '../../features/session/connectSteps';
import type {
  ConnectStep,
  ConnectStepKey,
} from '../../features/session/connectSteps';
import type { DialAttempt, NetPath } from '../../native/rscore/types';
import { radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';
import { useTitleFocus } from './useTitleFocus';

const OVERLAY = 'rgba(0,0,0,0.6)';
const PANEL_MAX_WIDTH = 452;
const PANEL_SIDE_INSET = 59;
const REFRESH_MS = 200;

type ActiveAttempt = DialAttempt & { state: 'trying' | 'ok' };

function isActiveAttempt(attempt: DialAttempt): attempt is ActiveAttempt {
  return attempt.state === 'trying' || attempt.state === 'ok';
}

function lastActiveAttempt(dial: readonly DialAttempt[]): ActiveAttempt | null {
  for (let index = dial.length - 1; index >= 0; index -= 1) {
    const attempt = dial[index];
    if (attempt !== undefined && isActiveAttempt(attempt)) {
      return attempt;
    }
  }
  return null;
}

export function connectPath({
  dial,
  host,
}: {
  dial: readonly DialAttempt[];
  host: SlotProps['ctx']['host'];
}): { path: NetPath; node: string; addr: string } | null {
  const attempt = lastActiveAttempt(dial);
  if (attempt !== null) {
    return {
      path: attempt.path,
      node: host?.node ?? '',
      addr: attempt.addr,
    };
  }
  if (host === undefined) {
    return null;
  }
  const address = host.addresses.find(item => item.kind === 'tailnet');
  if (address === undefined) {
    return null;
  }
  return { path: 'tailnetDirect', node: host.node, addr: address.addr };
}

function stepLabel(step: ConnectStep): string {
  const name = sessionConnect.steps[step.key].name;
  return sessionConnect.stepA11y(name, step.state, step.seconds);
}

function StepMarker({
  step,
  reduceMotion,
}: {
  step: ConnectStep;
  reduceMotion: boolean;
}) {
  const rotation = useRef(new Animated.Value(0)).current;
  const pulseStyle = useMemo(
    () => ({
      opacity: reduceMotion
        ? 1
        : rotation.interpolate({
            inputRange: [0, 1],
            outputRange: [0.65, 1],
          }),
    }),
    [reduceMotion, rotation],
  );

  useEffect(() => {
    if (step.state !== 'active' || reduceMotion) {
      rotation.setValue(0);
      return undefined;
    }
    const animation = Animated.loop(
      Animated.timing(rotation, {
        toValue: 1,
        duration: 1200,
        easing: Easing.linear,
        useNativeDriver: true,
      }),
    );
    animation.start();
    return () => animation.stop();
  }, [reduceMotion, rotation, step.state]);

  if (step.state === 'done') {
    return (
      <View style={[styles.marker, styles.markerDone]}>
        <Icon color="good" name="check" size={11} />
      </View>
    );
  }
  if (step.state === 'active') {
    return (
      <View style={[styles.marker, styles.markerActive]}>
        <Animated.View style={pulseStyle}>
          <View style={styles.activeDot} />
        </Animated.View>
      </View>
    );
  }
  return <View style={[styles.marker, styles.markerPending]} />;
}

function Connector({ state }: { state: ConnectStep['state'] }) {
  return (
    <View
      style={[
        styles.connector,
        state === 'done'
          ? styles.connectorDone
          : state === 'active'
          ? styles.connectorActive
          : styles.connectorPending,
      ]}
    />
  );
}

function StepItem({
  step,
  reduceMotion,
}: {
  step: ConnectStep;
  reduceMotion: boolean;
}) {
  const details = sessionConnect.steps[step.key];
  const seconds =
    step.seconds === null ? '—' : sessionConnect.stepSeconds(step.seconds);
  return (
    <View
      accessible
      accessibilityLabel={stepLabel(step)}
      accessibilityState={step.state === 'active' ? { busy: true } : undefined}
      style={styles.step}
      testID={`connect-step-${step.key}`}
    >
      <StepMarker reduceMotion={reduceMotion} step={step} />
      <View style={styles.stepCopy}>
        <AppText
          color={step.state === 'active' ? 'text' : 'text2'}
          variant="footnote"
          weight={step.state === 'active' ? 600 : 400}
        >
          {details.name}
        </AppText>
        <AppText
          color={step.state === 'active' ? 'accent' : 'text3'}
          mono
          variant="tiny"
        >
          {`${details.secondary} · ${seconds}`}
        </AppText>
      </View>
    </View>
  );
}

function Spinner({ reduceMotion }: { reduceMotion: boolean }) {
  const rotation = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    if (reduceMotion) {
      rotation.setValue(0);
      return undefined;
    }
    const animation = Animated.loop(
      Animated.timing(rotation, {
        toValue: 1,
        duration: 1000,
        easing: Easing.linear,
        useNativeDriver: true,
      }),
    );
    animation.start();
    return () => animation.stop();
  }, [reduceMotion, rotation]);

  const rotate = rotation.interpolate({
    inputRange: [0, 1],
    outputRange: ['0deg', '360deg'],
  });
  return (
    <Animated.View style={{ transform: [{ rotate }] }}>
      <Icon color="accent" name="spinner" size={16} />
    </Animated.View>
  );
}

function StuckNotice({
  stuck,
  elapsedS,
  os,
}: {
  stuck: ConnectStepKey;
  elapsedS: number;
  os: 'macos' | 'windows';
}) {
  const copy = sessionConnect.stuck[stuck];
  const detail = os === 'windows' ? copy.windowsDetail : copy.detail;
  return (
    <View
      accessible
      accessibilityLabel={sessionConnect.stuckA11y(detail, copy.hint)}
      accessibilityRole="status"
      style={styles.notice}
      testID="connect-stuck"
    >
      <Icon color="accent" name="info" size={15} />
      <View style={styles.noticeCopy}>
        <AppText style={styles.noticeTitle} weight={500}>
          {sessionConnect.stuck.elapsed(elapsedS, detail)}
        </AppText>
        <AppText color="text3" variant="caption">
          {copy.hint}
        </AppText>
      </View>
    </View>
  );
}

function SlowNotice({
  onRetry,
  onLowerQuality,
}: {
  onRetry: (() => void) | null;
  onLowerQuality: (() => void) | null;
}) {
  return (
    <View
      accessibilityRole="alert"
      style={styles.notice}
      testID="connect-slow"
    >
      <Icon color="accent" name="info" size={15} />
      <View style={styles.noticeCopy}>
        <AppText style={styles.noticeTitle} weight={500}>
          {sessionConnect.slow.title}
        </AppText>
        <AppText color="text3" variant="caption">
          {sessionConnect.slow.body}
        </AppText>
        <View style={styles.slowActions}>
          {onRetry === null ? null : (
            <Button
              accessibilityLabel={sessionConnect.slow.retry}
              onPress={onRetry}
              testID="connect-slow-retry"
              title={sessionConnect.slow.retry}
              variant="pill"
            />
          )}
          {onLowerQuality === null ? null : (
            <Button
              accessibilityLabel={sessionConnect.slow.lowerQuality}
              onPress={onLowerQuality}
              testID="connect-slow-quality"
              title={sessionConnect.slow.lowerQuality}
              variant="text"
            />
          )}
        </View>
      </View>
    </View>
  );
}

export type ConnectingOverlayProps = SlotProps & {
  /** 首帧迟迟不来时「重新连接」：关掉这次会话重开一次。 */
  onRetry?: () => void;
};

export function ConnectingOverlay({ ctx, onRetry }: ConnectingOverlayProps) {
  const { width, height: windowHeight } = useWindowSize();
  const [nowMs, setNowMs] = useState(() => Date.now());
  const title = sessionConnect.connectingTitle(ctx.hostName);
  const titleRef = useTitleFocus();
  const previousStuck = useRef<ConnectStepKey | null>(null);
  const largeText = PixelRatio.getFontScale() > 2;

  useEffect(() => {
    const timer = setInterval(() => setNowMs(Date.now()), REFRESH_MS);
    return () => clearInterval(timer);
  }, []);

  const result = connectSteps({
    phase: ctx.rec?.phase ?? null,
    startedAtMs: ctx.startedAtMs,
    phaseAtMs: ctx.phaseAtMs,
    firstFrameAtMs: ctx.firstFrameAtMs,
    nowMs,
  });

  useEffect(() => {
    const current = result.stuck;
    if (current === null) {
      previousStuck.current = null;
      return;
    }
    if (previousStuck.current === current) {
      return;
    }
    previousStuck.current = current;
    const copy = sessionConnect.stuck[current];
    const detail = ctx.os === 'windows' ? copy.windowsDetail : copy.detail;
    // There is no matching live region because the visible elapsed time changes every 200 ms.
    // Announce the stable stuck message once on both platforms instead.
    AccessibilityInfo.announceForAccessibility(
      sessionConnect.stuckA11y(detail, copy.hint),
    );
  }, [ctx.os, result.stuck]);
  const route = connectPath({
    dial: ctx.rec?.dial ?? [],
    host: ctx.host,
  });
  const panelWidth = Math.min(
    PANEL_MAX_WIDTH,
    Math.max(0, width - PANEL_SIDE_INSET * 2),
  );
  // 只说走的是哪条路，不显示节点名和 IP。
  const pathText = route === null ? '—' : common.path[route.path];
  const streamingAtMs = ctx.phaseAtMs.streaming;
  const slow =
    ctx.rec?.phase === 'streaming' &&
    ctx.firstFrameAtMs === null &&
    streamingAtMs !== undefined &&
    nowMs - streamingAtMs >= FIRST_FRAME_SLOW_MS;

  return (
    <View
      accessibilityViewIsModal
      style={styles.root}
      testID="connecting-overlay"
    >
      <View
        style={[
          styles.panel,
          { width: panelWidth, maxHeight: windowHeight - 24 },
        ]}
        testID="connecting-panel"
      >
        <ScrollView
          contentContainerStyle={styles.content}
          style={styles.scroll}
          testID="connecting-scroll"
        >
          <View style={styles.header}>
            <View style={styles.iconCircle}>
              <Spinner reduceMotion={ctx.reduceMotion} />
            </View>
            <View style={styles.headerCopy}>
              <View
                ref={titleRef as never}
                accessible
                accessibilityLabel={title}
                accessibilityRole="header"
              >
                <AppText style={styles.title} variant="headline" weight={600}>
                  {title}
                </AppText>
              </View>
              <View style={styles.pathRow}>
                <PathMark kind={route?.path ?? 'offline'} />
                <AppText color="text2" variant="caption">
                  {pathText}
                </AppText>
              </View>
            </View>
          </View>

          <View
            accessibilityLabel={sessionConnect.stepList}
            accessibilityRole="list"
            style={[styles.steps, largeText ? styles.stepsLarge : null]}
            testID="connect-steps"
          >
            {result.steps.map((step, index) => (
              <View
                key={step.key}
                style={[
                  styles.stepWrap,
                  largeText ? styles.stepWrapLarge : null,
                ]}
              >
                <StepItem reduceMotion={ctx.reduceMotion} step={step} />
                {index === result.steps.length - 1 || largeText ? null : (
                  <Connector state={step.state} />
                )}
              </View>
            ))}
          </View>

          {slow ? (
            <SlowNotice
              onLowerQuality={
                ctx.quality === 'eco' ? null : () => ctx.setQuality('eco')
              }
              onRetry={onRetry ?? null}
            />
          ) : result.stuck === null ? null : (
            <StuckNotice
              elapsedS={result.elapsedS}
              os={ctx.os}
              stuck={result.stuck}
            />
          )}
        </ScrollView>

        <View style={styles.footer}>
          <Button
            accessibilityLabel={sessionConnect.collapseA11y}
            onPress={ctx.collapse}
            testID="connect-collapse"
            title={sessionConnect.collapse}
            variant="text"
          />
          <Button
            accessibilityLabel={common.cancel}
            onPress={ctx.cancelConnect}
            title={common.cancel}
            variant="secondary"
          />
        </View>
      </View>
    </View>
  );
}

export default ConnectingOverlay;

const styles = StyleSheet.create({
  root: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: PANEL_SIDE_INSET,
    backgroundColor: OVERLAY,
  },
  panel: {
    paddingTop: 18,
    paddingHorizontal: 18,
    paddingBottom: 14,
    borderRadius: radius.panel,
    borderWidth: 1,
    borderColor: tokens.panelLine,
    backgroundColor: tokens.panel,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 12 },
    shadowOpacity: 0.45,
    shadowRadius: 28,
    elevation: 24,
  },
  scroll: { flexShrink: 1 },
  content: { gap: 14 },
  header: { flexDirection: 'row', alignItems: 'center', gap: 12 },
  iconCircle: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  headerCopy: { flex: 1, gap: 4 },
  title: { flexShrink: 1 },
  pathRow: { flexDirection: 'row', alignItems: 'center', gap: 5 },
  steps: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 14,
    backgroundColor: tokens.raised,
  },
  stepsLarge: { flexDirection: 'column', alignItems: 'stretch' },
  stepWrap: { flex: 1, flexDirection: 'row', alignItems: 'center' },
  stepWrapLarge: { flex: 0, width: '100%' },
  step: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 8 },
  stepCopy: { flexShrink: 1, gap: 1 },
  marker: {
    width: 20,
    height: 20,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
  },
  markerDone: { backgroundColor: tokens.goodSoft },
  markerActive: {
    borderWidth: 2,
    borderColor: tokens.accent,
    backgroundColor: tokens.raised,
  },
  markerPending: { borderWidth: 1, borderColor: tokens.inputLine },
  activeDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: tokens.accent,
  },
  connector: { width: 20, height: 2, marginHorizontal: 7, flexShrink: 0 },
  connectorDone: { backgroundColor: 'rgba(93,211,154,0.45)' },
  connectorActive: { backgroundColor: tokens.accent },
  connectorPending: { backgroundColor: tokens.inputLine },
  notice: {
    flexDirection: 'row',
    gap: 9,
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 12,
    backgroundColor: tokens.accentTint,
  },
  noticeCopy: { flex: 1, gap: 2 },
  noticeTitle: { fontSize: 12.5, lineHeight: 18 },
  slowActions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginTop: 6 },
  footer: { flexDirection: 'row', justifyContent: 'flex-end', gap: 8 },
});
