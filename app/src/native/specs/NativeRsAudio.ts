import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type AudioStats = {
  running: boolean;
  packets: CodegenTypes.Double;
  lost: CodegenTypes.Double;
  concealed: CodegenTypes.Double;
  underruns: CodegenTypes.Double;
  targetMs: CodegenTypes.Double;
  levelMs: CodegenTypes.Double;
  outputLatencyMs: CodegenTypes.Double;
  ioBufferMs: CodegenTypes.Double;
  route: string;
};

export type AudioStateEvent = {
  state: string;
  reason: string;
};

export interface Spec extends TurboModule {
  start(sessionId: string): Promise<boolean>;
  stop(): void;
  setMuted(muted: boolean): void;
  getStats(): Promise<AudioStats>;
  readonly onAudioState: CodegenTypes.EventEmitter<AudioStateEvent>;
}

export default TurboModuleRegistry.get<Spec>('RsAudio');
