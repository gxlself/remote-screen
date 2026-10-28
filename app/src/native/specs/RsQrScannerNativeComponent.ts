import { codegenNativeComponent } from 'react-native';
import type { CodegenTypes, HostComponent, ViewProps } from 'react-native';

export interface NativeProps extends ViewProps {
  active?: CodegenTypes.WithDefault<boolean, false>;
  torch?: CodegenTypes.WithDefault<boolean, false>;
  onCode?: CodegenTypes.DirectEventHandler<Readonly<{ text: string }>>;
  // starting | running | denied | restricted | unavailable | error
  onScannerState?: CodegenTypes.DirectEventHandler<
    Readonly<{ state: string; message: string; hasTorch: boolean }>
  >;
}

export default codegenNativeComponent<NativeProps>(
  'RsQrScannerView',
) as HostComponent<NativeProps>;
