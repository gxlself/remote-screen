/* eslint-disable no-bitwise -- modifiers uses the native ctrl/alt bit mask. */
import { useEffect, useRef, useState } from 'react';
import { KeyboardAvoidingView, Pressable, ScrollView, StyleSheet, Text } from 'react-native';
import { useCore } from '../rscore/CoreContext';
import type { SshId } from '../rscore/types';
import { TerminalView } from '../terminal/TerminalView';
import type { TerminalViewHandle } from '../terminal/TerminalView';
import type { HarnessProps } from './registry';

export default function TerminalHarness({ arg, log }: HarnessProps) {
  const core = useCore();
  const terminalRef = useRef<TerminalViewHandle>(null);
  const [sshId, setSshId] = useState<SshId | null>(null);
  const [mods, setMods] = useState(0);
  const [fontSize, setFontSize] = useState(13);
  const [keyboardActive, setKeyboardActive] = useState(false);

  useEffect(() => {
    let mounted = true;
    let opened: SshId | null = null;
    setSshId(null);
    if (arg) {
      core.ssh
        .open(arg, { tmux: false })
        .then(id => {
          if (!mounted) {
            core.ssh.close(id);
            return;
          }
          opened = id;
          setSshId(id);
          log('TERMVIEW-OPEN', { sshId: id });
        })
        .catch(error => {
          if (mounted) log('TERMVIEW-OPEN', { error: String(error) });
        });
    }
    return () => {
      mounted = false;
      if (opened !== null) core.ssh.close(opened);
    };
  }, [arg, core, log]);

  const button = (label: string, onPress: () => void, selected = false) => (
    <Pressable
      key={label}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      style={[styles.button, selected && styles.selected]}
      onPress={onPress}
    >
      <Text style={styles.buttonText}>{label}</Text>
    </Pressable>
  );

  return (
    <KeyboardAvoidingView style={styles.surface} behavior="padding">
      <TerminalView
        ref={terminalRef}
        sshId={sshId}
        fontSize={fontSize}
        theme="night"
        debugEcho={!arg}
        keyboardActive={keyboardActive}
        onActiveChange={setKeyboardActive}
        accessibilityLabel="终端输出"
        modifiers={mods}
        onModifiersConsumed={value => {
          setMods(0);
          log('TERMVIEW-MODS', { mods: value });
        }}
        onSizeChange={(cols, rows) => log('TERMVIEW-SIZE', { cols, rows })}
        onKeyboardFrame={frame =>
          log('TERMVIEW-KBD', { height: frame.height, hardware: frame.hardware })
        }
        onCopied={length => log('TERMVIEW-COPIED', { length })}
      />
      <ScrollView
        horizontal
        keyboardShouldPersistTaps="always"
        showsHorizontalScrollIndicator={false}
        style={styles.toolbarScroll}
        contentContainerStyle={styles.toolbar}
      >
        {button('键盘', () => setKeyboardActive(value => !value), keyboardActive)}
        {button('ctrl 单次', () => setMods(value => value ^ 1), (mods & 1) !== 0)}
        {button('alt 单次', () => setMods(value => value ^ 2), (mods & 2) !== 0)}
        {button('esc', () => terminalRef.current?.sendKey('esc'))}
        {button('↑', () => terminalRef.current?.sendKey('up'))}
        {button('粘贴', () => terminalRef.current?.paste())}
        {button('复制', () => terminalRef.current?.copySelection())}
        {button('A−', () => setFontSize(value => Math.max(9, value - 1)))}
        {button('A+', () => setFontSize(value => Math.min(24, value + 1)))}
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  surface: { flex: 1, backgroundColor: '#07090B' },
  toolbarScroll: { flexGrow: 0, flexShrink: 0 },
  toolbar: {
    flexDirection: 'row',
    gap: 6,
    paddingVertical: 6,
  },
  button: {
    minHeight: 32,
    paddingHorizontal: 10,
    justifyContent: 'center',
    borderRadius: 6,
    backgroundColor: '#1C2328',
  },
  buttonText: { color: '#D5DCE0', fontFamily: 'GeistMono-Regular', fontSize: 12 },
  selected: { backgroundColor: '#294D41' },
});
