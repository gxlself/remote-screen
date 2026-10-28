import { useEffect, useRef } from 'react';
import { AccessibilityInfo, Platform, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { addServer } from '../../i18n/zh/addServer';
import { radius, space, tokens } from '../../theme/tokens';
import type { TestState } from '../../features/ssh/serverForm';

const FAIL_REASONS = addServer.test.failReasons as Record<string, string>;

type Tone = 'muted' | 'accent' | 'good' | 'bad';

export function TestResult({
  state,
  idleHint,
}: {
  state: TestState;
  idleHint: string;
}) {
  const content = describe(state, idleHint);
  const dotColor =
    content.tone === 'muted' ? tokens.muted : tokens[content.tone];
  const titleColor =
    content.tone === 'muted'
      ? 'text2'
      : content.tone === 'accent'
      ? 'accent'
      : content.tone === 'good'
      ? 'good'
      : 'bad';
  const label =
    content.subtitle === null
      ? content.title
      : `${content.title}，${content.subtitle}`;
  const previousKind = useRef(state.kind);

  useEffect(() => {
    const previous = previousKind.current;
    previousKind.current = state.kind;
    if (
      Platform.OS === 'ios' &&
      previous !== state.kind &&
      (state.kind === 'ok' || state.kind === 'fail')
    ) {
      AccessibilityInfo.announceForAccessibility(label);
    }
  }, [label, state.kind]);

  return (
    <View
      accessibilityLiveRegion="polite"
      accessibilityLabel={label}
      accessible
      style={[styles.result, content.tone === 'good' ? styles.good : null]}
      testID="test-result"
    >
      <View style={[styles.dot, { backgroundColor: dotColor }]} />
      <View style={styles.copy}>
        <AppText color={titleColor} variant="bodyStrong">
          {content.title}
        </AppText>
        {content.subtitle === null ? null : (
          <AppText color="text2" variant="footnote">
            {content.subtitle}
          </AppText>
        )}
      </View>
    </View>
  );
}

function describe(
  state: TestState,
  idleHint: string,
): { tone: Tone; title: string; subtitle: string | null } {
  switch (state.kind) {
    case 'idle':
      return {
        tone: 'muted',
        title: addServer.test.idleTitle,
        subtitle: idleHint,
      };
    case 'running':
      return {
        tone: 'accent',
        title: addServer.test.runningTitle,
        subtitle: null,
      };
    case 'ok':
      return {
        tone: 'good',
        title:
          (state.via === null
            ? addServer.test.okTitle
            : addServer.test.okVia(state.via)) +
          (state.ms === null ? '' : addServer.test.okLatency(state.ms)),
        subtitle: addServer.test.okSubtitle,
      };
    case 'fail':
      return {
        tone: 'bad',
        title: addServer.test.failTitle,
        subtitle:
          FAIL_REASONS[state.reason] ?? addServer.test.failReasons.other,
      };
  }
}

const styles = StyleSheet.create({
  result: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: space.md,
    padding: space.md,
    borderRadius: radius.button,
    backgroundColor: tokens.card,
  },
  good: {
    backgroundColor: 'rgba(93, 211, 154, 0.08)',
  },
  dot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    marginTop: 6,
  },
  copy: {
    flex: 1,
    gap: 2,
  },
});

export default TestResult;
