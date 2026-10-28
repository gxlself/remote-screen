import { useCallback, useState } from 'react';
import { StyleSheet, Switch, Text, View } from 'react-native';
import type { NativeSyntheticEvent } from 'react-native';
import RsQrScannerView from '../specs/RsQrScannerNativeComponent';
import NativeRsDevLog from '../specs/NativeRsDevLog';
import { tokens } from '../../theme/tokens';

type ScannerCodeEvent = NativeSyntheticEvent<Readonly<{ text: string }>>;
type ScannerStateEvent = NativeSyntheticEvent<
  Readonly<{ state: string; message: string }>
>;

// The shared harness logger only accepts uppercase tags. Keep the scanner's
// requested mixed-case markers local, using the same native logging sink.
function recordScannerEvent(
  event: 'code' | 'state',
  payload: Record<string, unknown>,
) {
  const line = `RS-SCANNER-${event} ${JSON.stringify(payload)}`;
  NativeRsDevLog?.log?.(line);
  if (__DEV__) {
    console.log(line);
  }
}

export default function ScannerHarness() {
  const [active, setActive] = useState(true);
  const [state, setState] = useState('starting');
  const [codes, setCodes] = useState<string[]>([]);

  const onCode = useCallback((event: ScannerCodeEvent) => {
    const text = event.nativeEvent.text;
    const preview = text.slice(0, 16) + (text.length > 16 ? '…' : '');
    setCodes(previous => [preview, ...previous].slice(0, 5));
    recordScannerEvent('code', { code: preview, len: text.length });
  }, []);

  const onScannerState = useCallback((event: ScannerStateEvent) => {
    const next = event.nativeEvent.state;
    setState(next);
    recordScannerEvent('state', { state: next });
  }, []);

  return (
    <View style={styles.screen}>
      <RsQrScannerView
        active={active}
        onCode={onCode}
        onScannerState={onScannerState}
        style={StyleSheet.absoluteFill}
      />
      <View style={styles.panel}>
        <View style={styles.switchRow}>
          <Text style={styles.label}>{active ? '停止' : '开始'}</Text>
          <Switch
            accessibilityLabel={active ? '停止扫码' : '开始扫码'}
            onValueChange={setActive}
            value={active}
          />
        </View>
        <Text style={styles.label}>状态：{state}</Text>
        <Text style={styles.caption}>最近识别结果</Text>
        {codes.length === 0 ? (
          <Text style={styles.caption}>—</Text>
        ) : (
          codes.map((code, index) => (
            <Text key={index} style={styles.code}>
              {code}
            </Text>
          ))
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  panel: {
    position: 'absolute',
    left: 16,
    right: 16,
    bottom: 24,
    gap: 8,
    padding: 16,
    borderRadius: 16,
    backgroundColor: tokens.hud,
  },
  switchRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  label: {
    color: tokens.text,
    fontSize: 16,
  },
  caption: {
    color: tokens.text2,
    fontSize: 14,
  },
  code: {
    color: tokens.text,
    fontFamily: 'GeistMono-Regular',
    fontSize: 14,
  },
});
