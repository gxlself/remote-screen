import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Button,
  ScrollView,
  StatusBar,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';
import { zh } from '../../i18n/zh';
import { tokens } from '../../theme/tokens';

// RN supplies Performance at runtime; its 0.88 public types omit the global.
declare const performance: { now(): number };

type Core = typeof import('react-native-rs-core');
type AppProps = { autoConnect?: string };
type SelfTest = {
  status: 'running' | 'pass' | 'fail';
  ping?: string;
  pingMs?: number;
  slow?: string;
  slowMs?: number;
  eventCount?: number;
  sequence?: string[];
  eventWindowMs?: number;
  eventMeanGapMs?: number;
  eventsAfterStop?: number;
  elapsedMs?: number;
  error?: string;
};
const wait = (ms: number) =>
  new Promise<void>(resolve => setTimeout(resolve, ms));
const rounded = (value: number) => Math.round(value * 100) / 100;


function Content() {
  const [test, setTest] = useState<SelfTest>({ status: 'running' });
  const [events, setEvents] = useState<string[]>([]);
  const running = useRef(false);
  const runSelfTest = useCallback(async () => {
    if (running.current) {
      return;
    }
    running.current = true;
    setTest({ status: 'running' });
    setEvents([]);
    const started = performance.now();
    const result: SelfTest = { status: 'running' };
    let core: Core | undefined;
    try {
      // Loading here also turns missing native bindings into a visible result.
      core = require('react-native-rs-core') as Core;
      let then = performance.now();
      result.ping = core.ping();
      result.pingMs = rounded(performance.now() - then);
      then = performance.now();
      let timeout: ReturnType<typeof setTimeout> | undefined;
      try {
        result.slow = await Promise.race([
          core.slow(300),
          new Promise<never>((_, reject) => {
            timeout = setTimeout(
              () => reject(new Error('slow(300) timed out')),
              5000,
            );
          }),
        ]);
      } finally {
        if (timeout) {
          clearTimeout(timeout);
        }
      }
      result.slowMs = rounded(performance.now() - then);

      const sequence: string[] = [];
      const arrivals: number[] = [];
      let stopped = false;
      let eventsAfterStop = 0;
      core.startEvents({
        onEvent(seq: bigint, payload: string) {
          if (stopped) {
            eventsAfterStop += 1;
            return;
          }
          sequence.push(seq.toString());
          arrivals.push(performance.now());
          setEvents(previous => [...previous, `${seq}: ${payload}`]);
        },
      });
      then = performance.now();
      await wait(5000);
      result.eventWindowMs = rounded(performance.now() - then);
      core.stopEvents();
      stopped = true;
      await wait(750);
      result.eventsAfterStop = eventsAfterStop;
      result.eventCount = sequence.length;
      result.sequence = sequence;
      if (arrivals.length > 1) {
        result.eventMeanGapMs = rounded(
          (arrivals[arrivals.length - 1] - arrivals[0]) / (arrivals.length - 1),
        );
      }
      const ordered = sequence.every((value, index) => value === String(index));
      // A callback queued just before cancellation can still reach JS.
      result.status =
        result.ping === 'pong' &&
        result.slow.startsWith('slept_ms=') &&
        result.slowMs >= 280 &&
        sequence.length >= 9 &&
        sequence.length <= 12 &&
        ordered &&
        eventsAfterStop <= 1
          ? 'pass'
          : 'fail';
    } catch (error) {
      result.status = 'fail';
      result.error = String(error);
    } finally {
      try {
        core?.stopEvents();
      } catch (error) {
        result.status = 'fail';
        result.error = String(error);
      }
      result.elapsedMs = rounded(performance.now() - started);
      setTest(result);
      console.log(`RS-SELFTEST ${JSON.stringify(result)}`);
      running.current = false;
    }
  }, []);

  useEffect(() => {
    runSelfTest();
  }, [runSelfTest]);

  return (
    <SafeAreaView style={styles.screen}>
      <ScrollView
        contentContainerStyle={styles.content}
        keyboardShouldPersistTaps="handled"
      >
        <Text style={styles.title}>{zh.appName}</Text>
        <Text style={styles.subtitle}>
          {zh.dev.subtitle}
        </Text>
        <View style={styles.card}>
          <Text style={styles.heading}>{zh.dev.selfTest}: {test.status}</Text>
          <Text selectable testID="rs-selftest" style={styles.mono}>
            {JSON.stringify(test, null, 2)}
          </Text>
          <Text style={styles.mono}>
            {events.join('\n') || zh.dev.waiting}
          </Text>
          <Button
            title={zh.dev.runSelfTest}
            disabled={test.status === 'running'}
            onPress={() => { runSelfTest(); }}
          />
        </View>
      </ScrollView>
    </SafeAreaView>
  );
}

export default function SelfTest(_props: AppProps) {
  // Keep the launch entry's prop while session auto-connect is wired separately.
  return (
    <SafeAreaProvider>
      <StatusBar barStyle="light-content" />
      <Content />
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: tokens.bg },
  content: { padding: 20, gap: 16 },
  title: { fontFamily: 'Geist-Regular', fontSize: 25, fontWeight: '700', color: tokens.text },
  subtitle: { fontFamily: 'Geist-Regular', fontSize: 13, color: tokens.text2, lineHeight: 19 },
  card: { padding: 16, gap: 12, borderRadius: 12, backgroundColor: tokens.card },
  heading: { fontSize: 18, color: tokens.text, fontWeight: '600' },
  mono: { color: tokens.text, fontFamily: 'GeistMono-Regular', fontSize: 12, lineHeight: 18 },
});
