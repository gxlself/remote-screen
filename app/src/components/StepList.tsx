import { useEffect, useRef } from 'react';
import { Animated, Easing, StyleSheet, View } from 'react-native';
import type { ReactNode } from 'react';
import { common } from '../i18n/zh/common';
import { tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon } from './icons/Icon';

export type StepState = 'done' | 'active' | 'pending' | 'failed';

export type Step = {
  key: string;
  title: string;
  detail?: string;
  right?: ReactNode;
  extra?: ReactNode;
  state: StepState;
};

export type StepListProps = {
  steps: Step[];
  accessibilityLabel: string;
  reduceMotion?: boolean;
};

const CONNECTOR_DONE = 'rgba(93,211,154,0.35)';
const ACTIVE_RING = 'rgba(125,168,255,0.25)';

function stepAccessibilityLabel(step: Step): string {
  return [step.title, step.detail, common.step[step.state]]
    .filter(value => value != null && value !== '')
    .join('，');
}

function StepMarker({
  state,
  reduceMotion,
  testID,
}: {
  state: StepState;
  reduceMotion: boolean;
  testID: string;
}) {
  const rotation = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    if (state !== 'active' || reduceMotion) {
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

    return () => {
      animation.stop();
    };
  }, [reduceMotion, rotation, state]);

  if (state === 'done') {
    return (
      <View style={[styles.marker, styles.doneMarker]} testID={testID}>
        <Icon color="good" name="check" size={14} testID={`${testID}-icon`} />
      </View>
    );
  }

  if (state === 'failed') {
    return (
      <View style={[styles.marker, styles.failedMarker]} testID={testID}>
        <Icon color="bad" name="close" size={14} testID={`${testID}-icon`} />
      </View>
    );
  }

  if (state === 'active') {
    const rotate = rotation.interpolate({
      inputRange: [0, 1],
      outputRange: ['0deg', '360deg'],
    });

    return (
      <View style={[styles.marker, styles.activeMarker]} testID={testID}>
        <Animated.View style={{ transform: [{ rotate }] }}>
          <Icon
            color="accent"
            name="spinner"
            size={14}
            testID={`${testID}-icon`}
          />
        </Animated.View>
      </View>
    );
  }

  return <View style={[styles.marker, styles.pendingMarker]} testID={testID} />;
}

export function StepList({
  steps,
  accessibilityLabel,
  reduceMotion = false,
}: StepListProps) {
  return (
    <View accessibilityLabel={accessibilityLabel} style={styles.container}>
      {steps.map((step, index) => {
        const last = index === steps.length - 1;
        const hasSlot = step.right != null || step.extra != null;
        const label = stepAccessibilityLabel(step);
        const accessibilityState =
          step.state === 'active' ? { busy: true } : undefined;

        return (
          <View
            {...(hasSlot
              ? {}
              : {
                  accessible: true,
                  accessibilityLabel: label,
                  accessibilityState,
                })}
            key={step.key}
            style={styles.step}
            testID={`step-${step.key}${hasSlot ? '-row' : ''}`}
          >
            <View
              {...(hasSlot
                ? {
                    importantForAccessibility: 'no-hide-descendants' as const,
                    accessibilityElementsHidden: true,
                  }
                : {})}
              style={styles.markerColumn}
            >
              <StepMarker
                reduceMotion={reduceMotion}
                state={step.state}
                testID={`step-${step.key}-marker`}
              />
              {last ? null : (
                <View
                  style={[
                    styles.connector,
                    {
                      backgroundColor:
                        step.state === 'done'
                          ? CONNECTOR_DONE
                          : tokens.inputLine,
                    },
                  ]}
                  testID={`step-${step.key}-connector`}
                />
              )}
            </View>
            <View style={styles.body}>
              <View style={styles.headingRow}>
                <View
                  {...(hasSlot
                    ? {
                        accessible: true,
                        accessibilityLabel: label,
                        accessibilityState,
                        testID: `step-${step.key}`,
                      }
                    : {})}
                  style={styles.copy}
                >
                  <AppText
                    accessibilityLabel={
                      hasSlot && step.detail != null
                        ? `${step.title}，${common.step[step.state]}`
                        : undefined
                    }
                    accessible={
                      hasSlot && step.detail != null ? false : undefined
                    }
                    weight={step.state === 'active' ? 600 : 500}
                    variant="body"
                  >
                    {step.title}
                  </AppText>
                  {step.detail == null ? null : (
                    <AppText color="text3" variant="caption">
                      {step.detail}
                    </AppText>
                  )}
                </View>
                {step.right == null ? null : (
                  <View style={styles.right}>{step.right}</View>
                )}
              </View>
              {step.extra == null ? null : (
                <View style={styles.extra}>{step.extra}</View>
              )}
            </View>
          </View>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    borderRadius: 16,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
    paddingVertical: 6,
    paddingHorizontal: 16,
  },
  step: {
    flexDirection: 'row',
    gap: 12,
    paddingVertical: 12,
    paddingHorizontal: 0,
  },
  markerColumn: {
    width: 24,
    alignItems: 'center',
    alignSelf: 'stretch',
  },
  marker: {
    width: 24,
    height: 24,
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
  },
  doneMarker: {
    backgroundColor: tokens.goodSoft,
  },
  activeMarker: {
    borderWidth: 2,
    borderColor: ACTIVE_RING,
  },
  pendingMarker: {
    borderWidth: 2,
    borderColor: tokens.inputLine,
  },
  failedMarker: {
    backgroundColor: tokens.badSoft,
  },
  connector: {
    position: 'absolute',
    top: 28,
    bottom: -20,
    left: 11,
    width: 2,
    borderRadius: 1,
  },
  body: {
    flex: 1,
    gap: 6,
  },
  headingRow: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 12,
  },
  copy: {
    flex: 1,
    gap: 2,
  },
  right: {
    marginLeft: 'auto',
  },
  extra: {
    alignSelf: 'stretch',
  },
});
