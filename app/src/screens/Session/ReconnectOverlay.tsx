import { useEffect, useRef, useState } from 'react';
import {
  Animated,
  Easing,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { useSshServers } from '../../features/data/queries';
import { useNetwork } from '../../features/network/store';
import {
  formatReconnectClock,
  netChangedRecently,
  reconnectView,
  type ReconnectStepState,
} from '../../features/session/reconnectView';
import { common } from '../../i18n/zh/common';
import { localIdleReconnect } from '../../i18n/zh/localIdleReconnect';
import { radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';
import { useTitleFocus } from './useTitleFocus';

const COPY = localIdleReconnect.reconnect;
const REFRESH_MS = 200;
const DIALOG_MAX_WIDTH = 400;
/** 分屏或窄窗口时弹窗两边至少留的空。 */
const DIALOG_MARGIN = 16;

function MainSpinner({ reduceMotion }: { reduceMotion: boolean }) {
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
    <View style={styles.spinnerWell} testID="reconnect-spinner">
      <Animated.View style={{ transform: [{ rotate }] }}>
        <Icon color="accent" name="spinner" size={22} />
      </Animated.View>
    </View>
  );
}

function StepMark({ state }: { state: ReconnectStepState }) {
  if (state === 'done') {
    return (
      <View style={[styles.stepMark, styles.stepDone]}>
        <Icon color="good" name="check" size={12} />
      </View>
    );
  }
  if (state === 'active') {
    return (
      <View style={[styles.stepMark, styles.stepActive]}>
        <Icon color="accent" name="spinner" size={12} />
      </View>
    );
  }
  return <View style={[styles.stepMark, styles.stepPending]} />;
}

export function ReconnectOverlay({ ctx }: SlotProps) {
  const pausedAtMs =
    ctx.phaseAtMs.reconnecting ?? ctx.phaseAtMs.ended ?? ctx.startedAtMs;
  const [nowMs, setNowMs] = useState(() => Date.now());
  const { width: windowWidth, height: windowHeight } = useWindowSize();
  const networkUpdatedAtUs = useNetwork(state => state.updatedAtUs);
  const servers = useSshServers();
  const node = ctx.host?.node;
  // 「改用终端」要这台电脑真有 SSH 服务器才给。
  const hasTerminal =
    node !== undefined &&
    (servers.data ?? []).some(server => server.serverId === node);

  useEffect(() => {
    const interval = setInterval(() => setNowMs(Date.now()), REFRESH_MS);
    return () => clearInterval(interval);
  }, []);

  const model = reconnectView({
    reason: ctx.rec?.reconnectReason ?? null,
    ...(ctx.rec?.lastError?.reason === undefined
      ? {}
      : { endedReason: ctx.rec.lastError.reason }),
    os: ctx.os,
    dial: ctx.rec?.dial ?? [],
    sinceMs: pausedAtMs,
    nowMs,
    netChangedRecently: netChangedRecently(networkUpdatedAtUs, pausedAtMs),
    hasTerminal,
  });

  const titleRef = useTitleFocus();

  return (
    <View style={styles.overlay} testID="reconnect-overlay">
      <View
        accessibilityLabel={COPY.pausedAt(formatReconnectClock(pausedAtMs))}
        style={styles.pausedPill}
        testID="reconnect-paused"
      >
        <AppText color="text2" mono variant="monoSmall">
          {COPY.pausedAt(formatReconnectClock(pausedAtMs))}
        </AppText>
      </View>
      <View
        accessibilityViewIsModal
        style={[
          styles.dialog,
          {
            width: Math.min(
              DIALOG_MAX_WIDTH,
              Math.max(0, windowWidth - DIALOG_MARGIN * 2),
            ),
            maxHeight: windowHeight - 24,
          },
        ]}
        testID="reconnect-dialog"
      >
        <ScrollView
          contentContainerStyle={styles.content}
          style={styles.scroll}
        >
          <View style={styles.summary}>
            <MainSpinner reduceMotion={ctx.reduceMotion} />
            <View style={styles.copy}>
              <View
                ref={titleRef as never}
                accessible
                accessibilityLabel={model.title}
                accessibilityRole="header"
              >
                <AppText variant="headline" weight={600}>
                  {model.title}
                </AppText>
              </View>
              <AppText color="text2" variant="footnote">
                {model.desc}
              </AppText>
            </View>
          </View>
          <View style={styles.steps}>
            {model.steps.map(step => (
              <View
                accessible
                accessibilityLabel={
                  step.detail === null
                    ? `${step.label}，${common.step[step.state]}`
                    : `${step.label}，${common.step[step.state]}，${
                        step.detail
                      }`
                }
                accessibilityState={
                  step.state === 'active' ? { busy: true } : undefined
                }
                key={step.label}
                style={styles.step}
              >
                <StepMark state={step.state} />
                <View style={styles.stepCopy}>
                  <AppText
                    variant="footnote"
                    weight={step.state === 'active' ? 600 : 500}
                  >
                    {step.label}
                  </AppText>
                  {step.detail === null ? null : (
                    <AppText color="text3" variant="caption">
                      {step.detail}
                    </AppText>
                  )}
                </View>
              </View>
            ))}
          </View>
          <AppText color="text3" variant="caption">
            {model.waitedText}
          </AppText>
        </ScrollView>
        <View style={styles.actions}>
          <Button
            accessibilityLabel={COPY.collapseA11y}
            onPress={ctx.collapse}
            testID="reconnect-collapse"
            title={COPY.collapse}
            variant="text"
          />
          {model.showTerminal ? (
            <Button
              accessibilityLabel={COPY.useTerminal}
              onPress={ctx.openTerminal}
              title={COPY.useTerminal}
              variant="secondary"
            />
          ) : null}
          <Button
            accessibilityLabel={common.disconnect}
            onPress={ctx.disconnect}
            title={common.disconnect}
            variant="secondary"
          />
        </View>
      </View>
    </View>
  );
}

export default ReconnectOverlay;

const styles = StyleSheet.create({
  overlay: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(0,0,0,0.6)',
  },
  pausedPill: {
    position: 'absolute',
    top: 12,
    left: 59,
    minHeight: 32,
    justifyContent: 'center',
    paddingHorizontal: 12,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: tokens.hudLine,
    backgroundColor: tokens.hud,
  },
  dialog: {
    paddingTop: 18,
    paddingHorizontal: 18,
    paddingBottom: 14,
    borderRadius: radius.panel,
    borderWidth: 1,
    borderColor: tokens.panelLine,
    backgroundColor: tokens.panel,
  },
  scroll: { flexShrink: 1 },
  content: { gap: 14 },
  summary: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 14,
  },
  spinnerWell: {
    width: 40,
    height: 40,
    flexShrink: 0,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 20,
    borderWidth: 1,
    borderColor: tokens.accentSoft,
    backgroundColor: tokens.accentTint,
  },
  copy: {
    flex: 1,
    gap: 3,
  },
  steps: {
    gap: 10,
    paddingLeft: 4,
  },
  step: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 10,
  },
  stepCopy: {
    flex: 1,
    gap: 1,
  },
  stepMark: {
    width: 20,
    height: 20,
    flexShrink: 0,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 10,
  },
  stepDone: {
    backgroundColor: tokens.goodSoft,
  },
  stepActive: {
    borderWidth: 1,
    borderColor: tokens.accentSoft,
    backgroundColor: tokens.accentTint,
  },
  stepPending: {
    borderWidth: 1,
    borderColor: tokens.inputLine,
  },
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    justifyContent: 'flex-end',
    gap: 10,
  },
});
