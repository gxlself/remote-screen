import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type BonjourService = {
  name: string;
  /** 解析出的主机名（如 "Mac-mini.local"），解析失败为空串。 */
  host: string;
  port: CodegenTypes.Int32;
  /** IPv4 优先，最多 4 个。 */
  addrs: string[];
};
export type BonjourEvent = { kind: string; service: BonjourService }; // kind: 'found' | 'lost'

export interface Spec extends TurboModule {
  /** type 只接受 '_rfb._tcp'；重复 start 先停旧的。 */
  start(type: string): void;
  stop(): void;
  readonly onService: CodegenTypes.EventEmitter<BonjourEvent>;
}

export default TurboModuleRegistry.get<Spec>('RsBonjour');
