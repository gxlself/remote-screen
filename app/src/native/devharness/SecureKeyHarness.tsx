import { useEffect } from 'react';
import { Text } from 'react-native';
import { benchmarkSigning, getDeviceKeyInfo } from '../secureKey/secureKey';
import type { HarnessProps } from './registry';

export default function SecureKeyHarness({ log }: HarnessProps) {
  useEffect(() => {
    let mounted = true;
    (async () => {
      try {
        const info = await getDeviceKeyInfo();
        if (!mounted) return;
        log('SECUREKEY-INFO', info ?? { error: 'unavailable' });
        const benchmark = await benchmarkSigning(200);
        if (!mounted) return;
        log('SECUREKEY-BENCH', benchmark ?? { error: 'unavailable' });
      } catch (error) {
        if (!mounted) return;
        log('SECUREKEY-INFO', {
          error: error instanceof Error ? error.message : String(error),
        });
      }
    })();
    return () => {
      mounted = false;
    };
  }, [log]);

  return <Text>securekey</Text>;
}
