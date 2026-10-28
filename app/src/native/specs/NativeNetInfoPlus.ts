import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type NativeIface = {
  name: string;
  addrs: string[];
  isVpn: boolean;
  /** Android 工作资料里的接口；iOS 不填。 */
  workProfile?: boolean;
};

export type NativePath = {
  status: 'satisfied' | 'unsatisfied' | 'requiresConnection';
  interfaceTypes: Array<'wifi' | 'cellular' | 'wiredEthernet' | 'other'>;
  expensive: boolean;
  constrained: boolean;
};

export type NativeNetStatus = {
  ifaces: NativeIface[];
  path: NativePath;
  seq: CodegenTypes.Double;
};

export interface Spec extends TurboModule {
  setVpnPools(pools: string[]): void;
  getStatus(): Promise<NativeNetStatus>;
  readonly onChange: CodegenTypes.EventEmitter<NativeNetStatus>;
}

export default TurboModuleRegistry.get<Spec>('NetInfoPlus');
