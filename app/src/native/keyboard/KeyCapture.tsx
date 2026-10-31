import { StyleSheet } from 'react-native';
import { useCore } from '../rscore/CoreContext';
import RsKeyCapture from '../specs/RsKeyCaptureNativeComponent';

/** ⌃⌥⌘ on a hardware keyboard: kept on this device, never sent to the computer. */
export type SessionShortcutEvent =
  | { action: 'next' | 'previous' | 'switcher' | 'toolbar' }
  | { action: 'index'; index: number };

/** The native event as a shortcut; null for anything this build does not know. */
export function parseSessionShortcut(event: {
  action: string;
  index: number;
}): SessionShortcutEvent | null {
  switch (event.action) {
    case 'next':
    case 'previous':
    case 'switcher':
    case 'toolbar':
      return { action: event.action };
    case 'index':
      return Number.isInteger(event.index) && event.index >= 1
        ? { action: 'index', index: event.index }
        : null;
    default:
      return null;
  }
}

export type KeyCaptureProps = {
  sessionId: string | null;
  active: boolean;
  inputMode: 'text' | 'keys';
  stickyMods: number;
  os: 'macos' | 'windows';
  /** A hardware Cmd+V calls onPaste instead of reaching the computer. */
  syncPaste?: boolean;
  /** `mods`: the modifiers held on the keyboard, mapped for the computer. */
  onPaste?: (mods: number) => void;
  onStickyConsumed?: (mods: number) => void;
  onComposition?: (text: string) => void;
  onKeyboardFrame?: (frame: {
    height: number;
    visible: boolean;
    hardware: boolean;
  }) => void;
  onActiveChange?: (active: boolean) => void;
  onShortcut?: (event: SessionShortcutEvent) => void;
};

export function KeyCapture(p: KeyCaptureProps): React.JSX.Element {
  const core = useCore();

  return (
    <RsKeyCapture
      style={styles.capture}
      pointerEvents="none"
      accessible={false}
      importantForAccessibility="no"
      accessibilityElementsHidden
      active={p.active}
      inputMode={p.inputMode}
      stickyMods={p.stickyMods}
      os={p.os}
      syncPaste={p.syncPaste === true && p.onPaste !== undefined}
      onKey={({ nativeEvent: { usage, down, mods } }) => {
        if (p.sessionId === null) {
          return;
        }
        try {
          core.sessions.key(p.sessionId, usage, down, mods);
        } catch {
          core.logs.write('warn', 'keyboard', 'Failed to send key event');
        }
      }}
      onText={({ nativeEvent: { text } }) => {
        if (p.sessionId === null) {
          return;
        }
        try {
          core.sessions.text(p.sessionId, text);
        } catch {
          core.logs.write('warn', 'keyboard', 'Failed to send text event');
        }
      }}
      onComposition={({ nativeEvent: { text } }) => p.onComposition?.(text)}
      onStickyConsumed={({ nativeEvent: { mods } }) => p.onStickyConsumed?.(mods)}
      onKeyboardFrame={({ nativeEvent: { height, visible, hardware } }) =>
        p.onKeyboardFrame?.({ height, visible, hardware })
      }
      onActiveChange={({ nativeEvent: { active } }) => p.onActiveChange?.(active)}
      onPaste={({ nativeEvent: { mods } }) => p.onPaste?.(mods)}
      onShortcut={({ nativeEvent }) => {
        const shortcut = parseSessionShortcut(nativeEvent);
        if (shortcut !== null) {
          p.onShortcut?.(shortcut);
        }
      }}
    />
  );
}

const styles = StyleSheet.create({
  capture: {
    position: 'absolute',
    width: 1,
    height: 1,
    top: 0,
    left: 0,
  },
});
