import { useEffect, useState } from 'react';
import { Button, ScrollView, Text, TextInput } from 'react-native';
import type { HarnessProps } from './registry';
import type { NativeNetStatus } from '../specs/NativeNetInfoPlus';
import { getNativeNetwork, setVpnPools, subscribeNativeNetwork } from '../netinfo/netInfo';

const DEFAULT_POOL = '10.99.0.0/16';

export default function NetInfoHarness({ log }: HarnessProps) {
  const [status, setStatus] = useState<NativeNetStatus | null>(null);
  const [changes, setChanges] = useState(0);
  const [pool, setPool] = useState(DEFAULT_POOL);

  useEffect(() => {
    let mounted = true;
    getNativeNetwork().then(value => {
      if (mounted) {
        setStatus(value);
        if (value !== null) {
          log('NETINFO-STATUS', value);
        }
      }
    });
    const unsubscribe = subscribeNativeNetwork(value => {
      if (!mounted) {
        return;
      }
      setStatus(value);
      setChanges(count => {
        const next = count + 1;
        log('NETINFO-CHANGE', { seq: value.seq });
        return next;
      });
    });
    return () => {
      mounted = false;
      unsubscribe();
    };
  }, [log]);

  const applyPool = () => {
    const value = pool.trim();
    setVpnPools(value === '' ? [] : [value]);
  };

  return (
    <ScrollView>
      <Text>changes: {changes}</Text>
      <TextInput
        accessibilityLabel="VPN 地址池"
        onChangeText={setPool}
        onSubmitEditing={applyPool}
        value={pool}
      />
      <Button title="setVpnPools" onPress={applyPool} />
      <Text>
        {status === null ? 'status: null' : JSON.stringify(status, null, 2)}
      </Text>
    </ScrollView>
  );
}
