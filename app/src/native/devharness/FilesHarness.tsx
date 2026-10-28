import { useEffect, useState } from 'react';
import { Button, ScrollView, Text } from 'react-native';
import NativeRsDevLog from '../specs/NativeRsDevLog';
import NativeRsFiles from '../specs/NativeRsFiles';
import type { NativePickedFile } from '../specs/NativeRsFiles';

function marker(tag: string, payload: Record<string, unknown>): void {
  let encoded: string;
  try {
    encoded = JSON.stringify(payload);
  } catch {
    encoded = JSON.stringify({ error: 'unserializable' });
  }
  const line = `RS-${tag} ${encoded}`;
  NativeRsDevLog?.log?.(line);
  if (__DEV__) {
    console.log(line);
  }
}

export default function FilesHarness() {
  const [constants] = useState(() => {
    try {
      return NativeRsFiles?.getConstants() ?? null;
    } catch {
      return null;
    }
  });
  const [picked, setPicked] = useState<NativePickedFile[]>([]);
  const [lastResult, setLastResult] = useState<unknown>(null);
  const [backgroundActive, setBackgroundActive] = useState(false);

  useEffect(() => () => NativeRsFiles?.setTransfersActive(false), []);

  const pick = async () => {
    try {
      const files = (await NativeRsFiles?.pickFiles()) ?? [];
      setPicked(files);
      setLastResult(files);
      marker('FILES-pick', { files });
    } catch (error) {
      const result = { ok: false, error: String(error) };
      setLastResult(result);
      marker('FILES-pick', result);
    }
  };

  const save = async () => {
    const file = picked[picked.length - 1];
    if (file == null || NativeRsFiles == null) {
      const result = { ok: false, error: 'no file selected' };
      setLastResult(result);
      marker('FILES-save', result);
      return;
    }
    try {
      const ok = await NativeRsFiles.saveToFiles(file.uri, file.name);
      const result = { ok, name: file.name };
      setLastResult(result);
      marker('FILES-save', result);
    } catch (error) {
      const result = { ok: false, error: String(error) };
      setLastResult(result);
      marker('FILES-save', result);
    }
  };

  const setBackground = (active: boolean) => {
    setBackgroundActive(active);
    NativeRsFiles?.setTransfersActive(active);
    const result = { active };
    setLastResult(result);
    marker('FILES-bg', result);
  };

  return (
    <ScrollView>
      <Text>{JSON.stringify(constants, null, 2) ?? '—'}</Text>
      <Button
        title="选文件"
        onPress={() => {
          pick().catch(() => undefined);
        }}
      />
      <Button
        title="存到文件"
        onPress={() => {
          save().catch(() => undefined);
        }}
      />
      <Button
        title="后台开始"
        disabled={backgroundActive}
        onPress={() => setBackground(true)}
      />
      <Button
        title="后台结束"
        disabled={!backgroundActive}
        onPress={() => setBackground(false)}
      />
      <Text>picked: {JSON.stringify(picked)}</Text>
      <Text>lastResult: {JSON.stringify(lastResult) ?? '—'}</Text>
    </ScrollView>
  );
}
