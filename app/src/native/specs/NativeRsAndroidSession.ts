import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type ActiveSession = {
  sessionId: string;
  title: string;
  kind: string;
  startedAtMs: CodegenTypes.Double;
};

export type NotificationAction = {
  action: string;
  sessionId: string;
};

export type PipChange = {
  inPip: boolean;
};

export type TileConnect = {
  hostId: string;
};

export interface Spec extends TurboModule {
  setActiveSessions(sessions: ActiveSession[]): void;
  setPipEligible(
    eligible: boolean,
    aspectWidth: CodegenTypes.Int32,
    aspectHeight: CodegenTypes.Int32,
  ): void;
  isInPip(): boolean;
  setTileTarget(hostId: string, label: string): void;
  takePendingTileConnect(): string;
  readonly onNotificationAction: CodegenTypes.EventEmitter<NotificationAction>;
  readonly onPipChange: CodegenTypes.EventEmitter<PipChange>;
  readonly onTileConnect: CodegenTypes.EventEmitter<TileConnect>;
}

export default TurboModuleRegistry.get<Spec>('RsAndroidSession');
