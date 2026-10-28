import { StyleSheet, View } from 'react-native';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { onboarding } from '../../i18n/zh/onboarding';
import { space, tokens } from '../../theme/tokens';
import AppText from '../../components/AppText';

export type OnboardingStep = 'tailnet' | 'discover' | 'protect';

export type StepHeaderProps = {
  step: OnboardingStep;
};

/** The Tailscale step only exists while Tailscale is switched on. */
export function onboardingSteps(tailnetEnabled: boolean): OnboardingStep[] {
  return tailnetEnabled
    ? ['tailnet', 'discover', 'protect']
    : ['discover', 'protect'];
}

export function StepHeader({ step }: StepHeaderProps) {
  const steps = onboardingSteps(useTailnetEnabled());
  const index = Math.max(steps.indexOf(step), 0) + 1;
  const total = steps.length;
  const spokenLabel = onboarding.step.spoken(index, total);

  return (
    <View style={styles.header}>
      <View style={styles.spacer} />
      <AppText
        accessibilityLabel={spokenLabel}
        color="text3"
        style={styles.progress}
        variant="footnote"
      >
        {onboarding.step.progress(index, total)}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  header: {
    minHeight: 20,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: space.sm,
  },
  spacer: {
    flex: 1,
  },
  progress: {
    fontSize: 13,
    lineHeight: 18,
    textAlign: 'right',
    color: tokens.text3,
  },
});

export default StepHeader;
