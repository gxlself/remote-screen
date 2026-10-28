import { useEffect } from 'react';
import { Text } from 'react-native';
import type { HarnessProps } from './registry';

export default function PingHarness({ arg, log }: HarnessProps) {
  useEffect(() => {
    log('PING', { ok: true, arg: arg ?? null });
  }, [arg, log]);

  return <Text>ping</Text>;
}
