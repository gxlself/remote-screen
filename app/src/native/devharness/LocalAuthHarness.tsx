import { useEffect, useState } from 'react';
import { Button, ScrollView, Switch, Text, View } from 'react-native';
import type { HarnessProps } from './registry';
import {
  authenticate,
  biometryType,
  canAuthenticate,
  setPrivacyCover,
} from '../localAuth/localAuth';

export default function LocalAuthHarness({ log }: HarnessProps) {
  const [coverEnabled, setCoverEnabled] = useState(false);
  const [lastResult, setLastResult] = useState<string | null>(null);
  const capabilities = canAuthenticate();
  const type = biometryType();

  useEffect(() => () => setPrivacyCover(false), []);

  const record = (result: string) => {
    setLastResult(result);
    log('LOCALAUTH-RESULT', { result });
  };

  const verify = async () => {
    record(await authenticate('验证 Remote Screen 身份'));
  };

  const verifyTwice = async () => {
    const results = await Promise.all([
      authenticate('验证 Remote Screen 身份'),
      authenticate('验证 Remote Screen 身份'),
    ]);
    results.forEach(record);
  };

  const toggleCover = (enabled: boolean) => {
    setCoverEnabled(enabled);
    setPrivacyCover(enabled);
  };

  return (
    <ScrollView>
      <Text>biometryType: {type}</Text>
      <Text>canAuthenticate: {JSON.stringify(capabilities)}</Text>
      <Button title="验证" onPress={verify} />
      <Button title="连点两次验证" onPress={verifyTwice} />
      <View>
        <Text>多任务遮罩</Text>
        <Switch
          accessibilityLabel="多任务遮罩"
          value={coverEnabled}
          onValueChange={toggleCover}
        />
      </View>
      <Text>lastResult: {lastResult ?? '—'}</Text>
    </ScrollView>
  );
}
