import { useEffect, useState } from 'react';
import { Button, StyleSheet, Text, View } from 'react-native';
import { tokens } from '../../theme/tokens';
import { useCore } from '../rscore/CoreContext';
import RsKeyCapture from '../specs/RsKeyCaptureNativeComponent';
import type { HarnessProps } from './registry';

// The harness observes the raw native events so log-only mode also records
// key/text events. Product code uses keyboard/KeyCapture and never logs text.
export default function KeyboardHarness({ arg, log }: HarnessProps) {
  const core = useCore();
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [active, setActive] = useState(false);
  const [inputMode, setInputMode] = useState<'text' | 'keys'>('text');
  const [stickyMods, setStickyMods] = useState(0);
  const [windows, setWindows] = useState(false);
  const [composition, setComposition] = useState('');
  const [frame, setFrame] = useState({ height: 0, visible: false, hardware: false });

  useEffect(() => {
    let mounted = true;
    let opened: string | null = null;
    setSessionId(null);
    if (arg) {
      core.sessions.open(arg, { intent: 'control' }).then(
        id => {
          if (!mounted) {
            core.sessions.close(id);
            return;
          }
          opened = id;
          setSessionId(id);
          log('KEYCAP', { kind: 'session', status: 'opened' });
        },
        () => {
          if (mounted) {
            log('KEYCAP', { kind: 'session', status: 'failed' });
          }
        },
      );
    } else {
      log('KEYCAP', { kind: 'session', status: 'logOnly' });
    }
    return () => {
      mounted = false;
      if (opened !== null) {
        core.sessions.close(opened);
      }
    };
  }, [arg, core, log]);

  return (
    <View style={styles.panel}>
      <Text style={styles.status}>session: {sessionId ?? (arg ? 'opening / unavailable' : 'log only')}</Text>
      <Button title={active ? '收起' : '弹出'} onPress={() => setActive(!active)} />
      <Button
        title={inputMode === 'text' ? '文本' : '按键'}
        onPress={() => setInputMode(inputMode === 'text' ? 'keys' : 'text')}
      />
      <Button
        title={`粘滞 ⌘${stickyMods === 0 ? '' : ' ✓'}`}
        onPress={() => setStickyMods(stickyMods === 0 ? 0x08 : 0)}
      />
      <Button title={`Windows${windows ? ' ✓' : ''}`} onPress={() => setWindows(!windows)} />
      <Text style={styles.status}>
        {inputMode === 'text'
          ? '中文输入法可用 · 候选词在手机上选好再上屏'
          : '已切到英文布局 · 每次按键立即发送'}
      </Text>
      <Text style={styles.status}>composition: {composition || '—'}</Text>
      <Text style={styles.status}>keyboard height: {frame.height} pt · hardware: {String(frame.hardware)}</Text>
      <RsKeyCapture
        style={styles.capture}
        pointerEvents="none"
        accessible={false}
        importantForAccessibility="no"
        accessibilityElementsHidden
        active={active}
        inputMode={inputMode}
        stickyMods={stickyMods}
        os={windows ? 'windows' : 'macos'}
        onKey={({ nativeEvent: { usage, down, mods } }) => {
          log('KEYCAP', { kind: 'key', usage, down, mods });
          if (sessionId !== null) {
            try {
              core.sessions.key(sessionId, usage, down, mods);
            } catch {
              core.logs.write('warn', 'keyboard', 'Failed to send key event');
            }
          }
        }}
        onText={({ nativeEvent: { text } }) => {
          log('KEYCAP', { kind: 'text', text });
          if (sessionId !== null) {
            try {
              core.sessions.text(sessionId, text);
            } catch {
              core.logs.write('warn', 'keyboard', 'Failed to send text event');
            }
          }
        }}
        onComposition={({ nativeEvent: { text } }) => {
          setComposition(text);
          log('KEYCAP', { kind: 'composition', text });
        }}
        onStickyConsumed={({ nativeEvent: { mods } }) => {
          setStickyMods(0);
          log('KEYCAP', { kind: 'stickyConsumed', mods });
        }}
        onKeyboardFrame={({ nativeEvent: { height, visible, hardware } }) => {
          setFrame({ height, visible, hardware });
          log('KEYCAP', { kind: 'frame', height, hardware });
        }}
        onActiveChange={({ nativeEvent: { active: nextActive } }) => {
          setActive(nextActive);
          log('KEYCAP', { kind: 'active', active: nextActive });
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  panel: { gap: 8 },
  status: { color: tokens.text },
  capture: { position: 'absolute', width: 1, height: 1, top: 0, left: 0 },
});
