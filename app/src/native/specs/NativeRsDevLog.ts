import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  log(line: string): void;
}

export default TurboModuleRegistry.get<Spec>('RsDevLog');
