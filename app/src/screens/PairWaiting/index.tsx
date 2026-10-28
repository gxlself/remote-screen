import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Animated,
  Easing,
  Platform,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Countdown, useCountdown } from '../../components/Countdown';
import { Icon } from '../../components/icons/Icon';
import { IconButton } from '../../components/IconButton';
import { PathLabel } from '../../components/PathMark';
import { StepList } from '../../components/StepList';
import type { Step } from '../../components/StepList';
import { fpDisplay } from '../../features/format/fingerprint';
import { pairStatusText } from '../../features/pairing/announce';
import { describeFailure } from '../../features/pairing/outcome';
import type { FailureModel, PairMethod } from '../../features/pairing/outcome';
import { common } from '../../i18n/zh/common';
import { pairing } from '../../i18n/zh/pairing';
import { pairWaiting } from '../../i18n/zh/pairWaiting';
import type { PairOutcome } from '../../native/rscore/types';
import { tokens } from '../../theme/tokens';
import PairFailure from './PairFailure';
import PermissionNote from './PermissionNote';
import { isNotHostOutcome, usePairing } from './usePairing';
import type { PairHost } from './usePairing';

function useReduceMotion(): boolean {
  const [reduceMotion, setReduceMotion] = useState(false);

  useEffect(() => {
    let active = true;
    let changed = false;
    const subscription = AccessibilityInfo.addEventListener(
      'reduceMotionChanged',
      value => {
        changed = true;
        setReduceMotion(value);
      },
    );
    AccessibilityInfo.isReduceMotionEnabled()
      .then(value => {
        if (active && !changed) {
          setReduceMotion(value);
        }
      })
      .catch(() => {});
    return () => {
      active = false;
      subscription.remove();
    };
  }, []);

  return reduceMotion;
}

function PairDiagram({ reduceMotion }: { reduceMotion: boolean }) {
  const shift = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    if (reduceMotion) {
      shift.setValue(0);
      return undefined;
    }
    const animation = Animated.loop(
      Animated.timing(shift, {
        toValue: 1,
        duration: 900,
        easing: Easing.linear,
        useNativeDriver: true,
      }),
    );
    animation.start();
    return () => animation.stop();
  }, [reduceMotion, shift]);

  const translateX = shift.interpolate({
    inputRange: [0, 1],
    outputRange: [0, -12],
  });

  return (
    <View
      accessible={false}
      importantForAccessibility="no-hide-descendants"
      style={styles.diagram}
      testID="pair-diagram"
    >
      <View style={[styles.deviceTile, styles.phoneTile]}>
        <Icon color="accent" name="phone" size={26} />
      </View>
      <View style={styles.connector}>
        <View style={styles.dashClip}>
          <Animated.View
            style={[styles.dashLine, { transform: [{ translateX }] }]}
          />
        </View>
        <View style={styles.shield}>
          <Icon color="good" name="shield" size={15} />
        </View>
      </View>
      <View style={styles.deviceTile}>
        <Icon color="text2" name="computer" size={26} />
      </View>
    </View>
  );
}

function failureOf(
  outcome: PairOutcome,
  host: PairHost,
  method: PairMethod,
  byAddress: boolean,
): FailureModel {
  const failure = describeFailure(outcome, host.name, host.os, method);
  if (!byAddress || !isNotHostOutcome(outcome)) {
    return failure;
  }
  // The typed address is the likely mistake, so go back to the form.
  return {
    ...failure,
    message: pairWaiting.byAddress.notHost,
    primary: { label: pairing.actions.reenterCode, action: 'reenterCode' },
  };
}

export default function PairWaitingScreen({
  navigation,
  route,
}: RootScreenProps<'PairWaiting'>) {
  const insets = useSafeAreaInsets();
  const reduceMotion = useReduceMotion();
  const { state, steps, host, method, address, cancel, restart } = usePairing(
    route.params,
    navigation,
  );
  const hostWord = pairing.hostWord(host.os);
  const countdownKey = `${state.step ?? 'starting'}:${state.phase}`;
  const remaining = useCountdown(state.timeoutS, {
    running: state.phase === 'progress' && state.step === 'waitingLocal',
    key: countdownKey,
  });

  const failure =
    state.phase === 'failed' &&
    state.outcome !== null &&
    state.outcome.status !== 'cancelled'
      ? failureOf(state.outcome, host, method, address !== null)
      : null;
  const statusText = pairStatusText({
    failure,
    hostWord,
    phase: state.phase,
    step: state.step,
  });
  const previousStatusText = useRef<string | null>(null);
  useEffect(() => {
    if (statusText === null) {
      previousStatusText.current = null;
      return;
    }
    if (statusText === previousStatusText.current) {
      return;
    }
    previousStatusText.current = statusText;
    if (Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(statusText);
    }
  }, [statusText]);
  const displayFp = state.fp === null ? null : fpDisplay(state.fp);

  const stepRows: Step[] = [
    {
      key: 'connect',
      title: pairing.steps.connect.title,
      detail:
        address !== null && steps[0].state === 'active'
          ? pairWaiting.byAddress.probing
          : undefined,
      right:
        state.path !== null && state.latencyMs !== null ? (
          <PathLabel kind={state.path} latencyMs={state.latencyMs} />
        ) : undefined,
      state: steps[0].state,
    },
    {
      key: 'verify',
      title: pairing.steps.verify.title,
      right:
        steps[1].state === 'done' ? (
          <AppText color="good" variant="caption">
            {pairWaiting.verifyMatched(hostWord)}
          </AppText>
        ) : undefined,
      extra:
        displayFp === null ? undefined : (
          <View
            accessible
            accessibilityLabel={pairWaiting.fingerprint(displayFp)}
            style={styles.fingerprint}
            testID="pair-fingerprint"
          >
            <AppText mono variant="mono">
              {displayFp}
            </AppText>
          </View>
        ),
      state: steps[1].state,
    },
    ...(method === 'file'
      ? []
      : [
          {
            key: 'approve',
            title: pairWaiting.approveTitle(hostWord),
            detail: pairWaiting.approveDetail(hostWord),
            right:
              steps[2].state === 'active' ? (
                <Countdown remaining={remaining} total={state.timeoutS} />
              ) : undefined,
            state: steps[2].state,
          },
        ]),
  ];

  const primaryFailure = () => {
    if (failure === null) {
      return;
    }
    switch (failure.primary.action) {
      case 'reenterCode':
        if (address !== null) {
          navigation.replace('PairAdd', { tab: 'code', ...address });
        } else if (host.hostId !== null) {
          navigation.replace('PairAdd', {
            tab: 'code',
            hostId: host.hostId,
            ...(host.address === null ? {} : { addr: host.address }),
          });
        } else {
          navigation.replace('PairAdd', { tab: 'code' });
        }
        return;
      case 'rescan':
        navigation.replace('PairAdd', { tab: 'scan' });
        return;
      case 'reimportFile':
        navigation.replace('PairFile');
        return;
      case 'otherMethod':
        navigation.replace(
          'PairAdd',
          host.hostId === null
            ? undefined
            : {
                tab: 'code',
                hostId: host.hostId,
                ...(host.address === null ? {} : { addr: host.address }),
              },
        );
        return;
      case 'retry':
        restart();
        return;
      case 'close':
        navigation.goBack();
        return;
    }
  };
  return (
    <View
      style={[styles.screen, { paddingTop: insets.top + 8 }]}
      testID="pair-waiting-screen"
    >
      <View style={styles.header}>
        <IconButton
          accessibilityLabel={pairWaiting.a11y.cancel}
          icon="close"
          onPress={cancel}
          size={44}
          testID="pair-waiting-cancel-icon"
          variant="raised"
        />
        <AppText
          accessibilityRole="header"
          variant="headline"
          weight={600}
          style={styles.headerTitle}
        >
          {pairWaiting.header.title}
        </AppText>
        <View style={styles.headerSpacer} />
      </View>
      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: insets.bottom + 24 },
        ]}
        showsVerticalScrollIndicator={false}
      >
        {failure === null ? (
          <>
            <PairDiagram reduceMotion={reduceMotion} />
            <View style={styles.hero}>
              <AppText
                accessibilityRole="header"
                center
                variant="title"
                weight={700}
                style={styles.heroTitle}
              >
                {pairWaiting.title(host.name)}
              </AppText>
              <AppText center color="text3" mono variant="monoSmall">
                {pairWaiting.subtitle(
                  host.node ?? '',
                  host.address,
                  host.hostVersion,
                )}
              </AppText>
            </View>
          </>
        ) : null}

        <StepList
          accessibilityLabel={pairWaiting.progress}
          reduceMotion={reduceMotion}
          steps={stepRows}
        />

        {failure === null ? (
          <>
            <PermissionNote hostWord={hostWord} />
            <View style={styles.footer}>
              <Button
                fullWidth
                onPress={cancel}
                title={common.cancel}
                variant="secondary"
              />
              {method === 'file' ? null : (
                <AppText center color="text3" variant="caption">
                  {pairWaiting.timeoutHint(
                    hostWord,
                    state.timeoutS,
                    method === 'qr' ? 'qr' : 'code',
                  )}
                </AppText>
              )}
            </View>
          </>
        ) : (
          <PairFailure
            failure={failure}
            onClose={() => navigation.goBack()}
            onPrimary={primaryFailure}
          />
        )}
        {Platform.OS === 'android' && statusText !== null ? (
          <AppText accessibilityLiveRegion="polite" style={styles.statusLive}>
            {statusText}
          </AppText>
        ) : null}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: tokens.bg },
  header: {
    minHeight: 52,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 16,
  },
  headerTitle: { flex: 1, textAlign: 'center' },
  headerSpacer: { width: 44 },
  content: { paddingTop: 12, paddingHorizontal: 20, gap: 22 },
  diagram: {
    alignSelf: 'center',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingVertical: 6,
  },
  deviceTile: {
    width: 56,
    height: 56,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
    alignItems: 'center',
    justifyContent: 'center',
  },
  phoneTile: { borderColor: tokens.accent },
  connector: {
    width: 84,
    height: 56,
    alignItems: 'center',
    justifyContent: 'center',
  },
  dashClip: { width: '100%', height: 2, overflow: 'hidden' },
  dashLine: {
    width: 120,
    height: 2,
    borderTopWidth: 1,
    borderStyle: 'dashed',
    borderColor: tokens.text3,
    marginLeft: -6,
  },
  shield: {
    position: 'absolute',
    width: 24,
    height: 24,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.goodSoft,
  },
  hero: { alignItems: 'center', gap: 6 },
  heroTitle: { fontSize: 24, lineHeight: 30 },
  fingerprint: {
    minHeight: 30,
    borderRadius: 8,
    backgroundColor: tokens.raised,
    justifyContent: 'center',
    paddingHorizontal: 10,
  },
  footer: { gap: 10 },
  statusLive: {
    position: 'absolute',
    width: 1,
    height: 1,
    opacity: 0,
  },
});
