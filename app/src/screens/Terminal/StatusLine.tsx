import { useEffect, useRef } from 'react';
import { AccessibilityInfo, Platform, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { failureKind } from '../../features/terminal/tabs';
import type { TermStatus } from '../../features/terminal/tabs';
import { terminal } from '../../i18n/zh/terminal';
import { hit, space, tokens } from '../../theme/tokens';

export const terminalTone = {
  good: tokens.good,
  warn: tokens.relay,
  bad: tokens.bad,
  muted: tokens.muted,
} as const;

export function StatusLine(p: { status: TermStatus; onReconnect: () => void }) {
  const { status } = p;
  // The raw core message stays out of the UI; it only picks the next step.
  const failure = failureKind(status.message);
  const labels = {
    opening: terminal.connecting,
    connecting: terminal.connecting,
    authenticating: terminal.authenticating,
    check: terminal.check,
    open:
      (status.auth === null ? terminal.ssh : terminal.auth[status.auth]) +
      (status.jump === null ? '' : terminal.via(status.jump)),
    failed: failure === null ? terminal.failed : terminal.failure[failure],
    closed: terminal.closed,
  };
  const label = labels[status.phase];
  const previousLabel = useRef(label);

  useEffect(() => {
    if (previousLabel.current !== label && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(label);
    }
    previousLabel.current = label;
  }, [label]);

  const androidReconnect = Platform.OS === 'android' && status.canReconnect;
  return (
    <View
      style={[styles.line, androidReconnect ? { minHeight: hit.min } : null]}
      testID="terminal-status"
    >
      <View style={styles.left}>
        <View
          style={[styles.dot, { backgroundColor: terminalTone[status.tone] }]}
        />
        <AppText
          accessibilityLiveRegion="polite"
          color="text2"
          style={styles.text}
          testID="terminal-status-text"
          variant="caption"
        >
          {label}
        </AppText>
        {status.canReconnect ? (
          <View
            style={[
              styles.reconnect,
              androidReconnect ? { minHeight: hit.min } : null,
            ]}
          >
            <Button
              accessibilityLabel={terminal.reconnect}
              onPress={p.onReconnect}
              title={terminal.reconnect}
              variant="text"
            />
          </View>
        ) : null}
      </View>
      {status.tmux ? (
        <AppText color="text3" style={styles.tmux} variant="caption">
          {status.resumed ? terminal.tmuxResumed : terminal.tmux}
        </AppText>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  line: {
    minHeight: 28,
    paddingHorizontal: 14,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: space.sm,
  },
  left: { flexShrink: 1, flexDirection: 'row', alignItems: 'center', gap: 6 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  text: { flexShrink: 1 },
  reconnect: { minHeight: 28, justifyContent: 'center' },
  tmux: { flexShrink: 1 },
});
