import { codegenNativeCommands, codegenNativeComponent } from 'react-native';
import type React from 'react';
import type { CodegenTypes, HostComponent, ViewProps } from 'react-native';

interface CodegenNativeProps extends ViewProps {
  sshId?: CodegenTypes.WithDefault<string, ''>;
  fontSize?: CodegenTypes.WithDefault<CodegenTypes.Float, 13>;
  // RN 0.88 codegen cannot parse a single string literal enum. Use string
  // storage internally and retain the exact public type below.
  theme?: CodegenTypes.WithDefault<string, 'night'>;
  modifiers?: CodegenTypes.WithDefault<CodegenTypes.Int32, 0>;
  keyboardActive?: CodegenTypes.WithDefault<boolean, false>;
  debugEcho?: CodegenTypes.WithDefault<boolean, false>;
  onModifiersConsumed?: CodegenTypes.DirectEventHandler<
    Readonly<{ mods: CodegenTypes.Int32 }>
  >;
  onSizeChange?: CodegenTypes.DirectEventHandler<
    Readonly<{ cols: CodegenTypes.Int32; rows: CodegenTypes.Int32 }>
  >;
  onKeyboardFrame?: CodegenTypes.DirectEventHandler<
    Readonly<{
      height: CodegenTypes.Float;
      visible: boolean;
      hardware: boolean;
    }>
  >;
  onActiveChange?: CodegenTypes.DirectEventHandler<
    Readonly<{ active: boolean }>
  >;
  onCopied?: CodegenTypes.DirectEventHandler<
    Readonly<{ length: CodegenTypes.Int32 }>
  >;
}

export interface NativeProps extends CodegenNativeProps {
  theme?: CodegenTypes.WithDefault<'night', 'night'>;
}

export interface NativeCommands {
  sendSequence(
    ref: React.ElementRef<HostComponent<NativeProps>>,
    text: string,
  ): void;
  sendKey(
    ref: React.ElementRef<HostComponent<NativeProps>>,
    name: string,
  ): void;
  copySelection(ref: React.ElementRef<HostComponent<NativeProps>>): void;
  paste(ref: React.ElementRef<HostComponent<NativeProps>>): void;
}

export const Commands = codegenNativeCommands<NativeCommands>({
  supportedCommands: ['sendSequence', 'sendKey', 'copySelection', 'paste'],
});

export default codegenNativeComponent<CodegenNativeProps>(
  'RsTerminalView',
) as HostComponent<NativeProps>;
