import { useImperativeHandle, useRef } from 'react';
import type React from 'react';
import { StyleSheet } from 'react-native';
import type { StyleProp, ViewStyle } from 'react-native';
import type { SshId } from '../rscore/types';
import NativeRsTerminalView, {
  Commands,
} from '../specs/RsTerminalViewNativeComponent';

export type TermKeyName =
  | 'esc'
  | 'tab'
  | 'up'
  | 'down'
  | 'left'
  | 'right'
  | 'home'
  | 'end'
  | 'pageUp'
  | 'pageDown';

export type TerminalHandle = {
  sendSequence(text: string): void;
  copySelection(): void;
  paste(): void;
};

export type TerminalViewHandle = TerminalHandle & {
  sendKey(name: TermKeyName | string): void;
};

export type TerminalViewProps = {
  sshId: SshId | null;
  fontSize: number;
  theme: 'night';
  accessibilityLabel: string;
  modifiers?: number;
  keyboardActive?: boolean;
  debugEcho?: boolean;
  onModifiersConsumed?: (mods: number) => void;
  onSizeChange?: (cols: number, rows: number) => void;
  onKeyboardFrame?: (frame: {
    height: number;
    visible: boolean;
    hardware: boolean;
  }) => void;
  onActiveChange?: (active: boolean) => void;
  onCopied?: (length: number) => void;
  style?: StyleProp<ViewStyle>;
  ref?: React.Ref<TerminalViewHandle>;
};

export function TerminalView(p: TerminalViewProps): React.JSX.Element {
  const nativeRef =
    useRef<React.ElementRef<typeof NativeRsTerminalView>>(null);
  useImperativeHandle(
    p.ref,
    () => ({
      sendSequence(text) {
        if (nativeRef.current) Commands.sendSequence(nativeRef.current, text);
      },
      sendKey(name) {
        if (nativeRef.current) Commands.sendKey(nativeRef.current, name);
      },
      copySelection() {
        if (nativeRef.current) Commands.copySelection(nativeRef.current);
      },
      paste() {
        if (nativeRef.current) Commands.paste(nativeRef.current);
      },
    }),
    [],
  );

  return (
    <NativeRsTerminalView
      ref={nativeRef}
      style={[styles.surface, p.style]}
      accessible
      accessibilityLabel={p.accessibilityLabel}
      sshId={p.sshId ?? ''}
      fontSize={p.fontSize}
      theme={p.theme}
      modifiers={p.modifiers ?? 0}
      keyboardActive={p.keyboardActive ?? false}
      debugEcho={p.debugEcho ?? false}
      onModifiersConsumed={e => p.onModifiersConsumed?.(e.nativeEvent.mods)}
      onSizeChange={e =>
        p.onSizeChange?.(e.nativeEvent.cols, e.nativeEvent.rows)
      }
      onKeyboardFrame={e => p.onKeyboardFrame?.(e.nativeEvent)}
      onActiveChange={e => p.onActiveChange?.(e.nativeEvent.active)}
      onCopied={e => p.onCopied?.(e.nativeEvent.length)}
    />
  );
}

const styles = StyleSheet.create({
  surface: { flex: 1, backgroundColor: '#07090B' },
});
