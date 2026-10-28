import { useEffect, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import AppProviders from '../../app/AppProviders';
import { tokens } from '../../theme/tokens';
import { harnessLog } from './log';
import { harnesses } from './registry';

export type DevHarnessProps = {
  name: string;
  arg?: string;
};

export default function DevHarness({ name, arg }: DevHarnessProps) {
  const [ready, setReady] = useState(false);
  const Harness = harnesses[name];

  useEffect(() => {
    if (Harness) {
      harnessLog('HARNESS', { name, ready: true });
      setReady(true);
    } else {
      harnessLog('HARNESS', { name, error: 'unknown' });
    }
  }, [Harness, name]);

  return (
    <AppProviders>
      <View style={styles.screen}>
        <Text style={styles.header}>
          {name}
          {arg === undefined ? '' : ` ${arg}`}
        </Text>
        {Harness && ready ? (
          <Harness arg={arg} log={harnessLog} />
        ) : (
          <Text style={styles.message}>未知的 harness：{name}</Text>
        )}
      </View>
    </AppProviders>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
    padding: 20,
  },
  header: {
    color: tokens.text,
    fontFamily: 'GeistMono-Regular',
    fontSize: 16,
    marginBottom: 20,
  },
  message: {
    color: tokens.text,
    fontSize: 16,
  },
});
