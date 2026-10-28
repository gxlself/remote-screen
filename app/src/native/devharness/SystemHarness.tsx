import { useMemo, useState } from 'react';
import { Button, Linking, ScrollView, Share, Text } from 'react-native';
import type { HarnessProps } from './registry';
import NativeRsSystem from '../specs/NativeRsSystem';
import {
  appVersion,
  copyText,
  haptic,
  maxFps,
  modelId,
  readText,
  requestLocalNetwork,
  requestNotifications,
} from '../platform/impl/system';
import { useCore } from '../rscore/CoreContext';

function constants() {
  return NativeRsSystem?.getConstants() ?? {
    appVersion: appVersion(),
    buildNumber: '0',
    bundleId: 'unknown',
    modelId: modelId(),
    maxFps: maxFps(),
  };
}

export default function SystemHarness({ log }: HarnessProps) {
  const core = useCore();
  const [lastRead, setLastRead] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState('—');
  const values = useMemo(() => constants(), []);

  const record = (action: string, payload: Record<string, unknown>) => {
    setLastResult(JSON.stringify(payload));
    log(`SYSTEM-${action}`, payload);
  };

  const copy = async () => {
    await copyText('hello');
    record('COPY', { ok: true });
  };

  const read = async () => {
    const value = await readText();
    setLastRead(value);
    // Clipboard contents stay on screen only; never put them in logs.
    record('READ', {
      hasText: value !== null,
      length: value?.length ?? 0,
    });
  };

  const runHaptics = () => {
    (['light', 'success', 'warning', 'error'] as const).forEach(haptic);
    record('HAPTIC', { kinds: ['light', 'success', 'warning', 'error'] });
  };

  const notify = async () => {
    const result = await requestNotifications();
    record('NOTIFICATIONS', { result });
  };

  const localNetwork = async () => {
    const result = await requestLocalNetwork();
    record('LOCAL-NETWORK', { result });
  };

  const shareTestFile = async () => {
    try {
      // The native exporter writes a small masked zip into the app Caches
      // directory, giving the iPad check a real file URL to share.
      const path = await core.logs.export({ mask: true });
      const url = `file://${path}`;
      await Share.share({ url, title: 'Remote Screen test file' });
      record('SHARE', { url, ok: true });
    } catch (error) {
      record('SHARE', { ok: false, error: String(error) });
    }
  };

  const checkUrls = async () => {
    const [vnc, rdp] = await Promise.all([
      Linking.canOpenURL('vnc://100.64.0.7'),
      Linking.canOpenURL('rdp://full%20address=s:100.64.0.7'),
    ]);
    record('URLS', { vnc, rdp });
  };

  const openTestFlight = async () => {
    try {
      await Linking.openURL('itms-beta://');
      record('TESTFLIGHT', { ok: true });
    } catch (error) {
      record('TESTFLIGHT', { ok: false, error: String(error) });
    }
  };

  return (
    <ScrollView>
      <Text>{JSON.stringify(values, null, 2)}</Text>
      <Button title="复制 hello" onPress={copy} />
      <Button title="读剪贴板" onPress={read} />
      <Button title="四种触感" onPress={runHaptics} />
      <Button title="通知授权" onPress={notify} />
      <Button title="本地网络授权" onPress={localNetwork} />
      <Button title="分享测试文件" onPress={shareTestFile} />
      <Button title="检查 vnc:// 和 rdp://" onPress={checkUrls} />
      <Button title="打开 TestFlight" onPress={openTestFlight} />
      <Text>lastRead: {lastRead ?? '—'}</Text>
      <Text>lastResult: {lastResult}</Text>
    </ScrollView>
  );
}
