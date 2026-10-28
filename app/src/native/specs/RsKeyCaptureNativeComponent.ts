import { codegenNativeComponent } from 'react-native';
import type { CodegenTypes, HostComponent, ViewProps } from 'react-native';

export interface NativeProps extends ViewProps {
  active?: CodegenTypes.WithDefault<boolean, false>;
  inputMode?: CodegenTypes.WithDefault<'text' | 'keys', 'text'>;
  stickyMods?: CodegenTypes.WithDefault<CodegenTypes.Int32, 0>;
  os?: CodegenTypes.WithDefault<'macos' | 'windows', 'macos'>;
  /** Cmd+V waits for JS to push the clipboard (onPaste) instead of going out. */
  syncPaste?: CodegenTypes.WithDefault<boolean, false>;
  onKey?: CodegenTypes.DirectEventHandler<
    Readonly<{ usage: CodegenTypes.Int32; down: boolean; mods: CodegenTypes.Int32 }>
  >;
  onText?: CodegenTypes.DirectEventHandler<Readonly<{ text: string }>>;
  onComposition?: CodegenTypes.DirectEventHandler<Readonly<{ text: string }>>;
  onStickyConsumed?: CodegenTypes.DirectEventHandler<
    Readonly<{ mods: CodegenTypes.Int32 }>
  >;
  onKeyboardFrame?: CodegenTypes.DirectEventHandler<
    Readonly<{ height: CodegenTypes.Float; visible: boolean; hardware: boolean }>
  >;
  onActiveChange?: CodegenTypes.DirectEventHandler<Readonly<{ active: boolean }>>;
  onPaste?: CodegenTypes.DirectEventHandler<Readonly<{ mods: CodegenTypes.Int32 }>>;
  /**
   * ⌃⌥⌘ session shortcuts, kept on this device. action: next | previous |
   * index (index = 1…9) | switcher.
   */
  onShortcut?: CodegenTypes.DirectEventHandler<
    Readonly<{ action: string; index: CodegenTypes.Int32 }>
  >;
}

export default codegenNativeComponent<NativeProps>(
  'RsKeyCapture',
) as HostComponent<NativeProps>;
