import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type PowerState = {
  lowPower: boolean;
  lowPowerMode: boolean;
  batteryLevel: CodegenTypes.Double;
  charging: boolean;
};

export type BackgroundTaskEvent = {
  event: string;
  reason: string;
  heldMs: CodegenTypes.Double;
};

export interface Spec extends TurboModule {
  getConstants(): { previousRunEndedInBackground: boolean };
  setIdleTimerDisabled(disabled: boolean): void;
  setBackgroundKeepAlive(enabled: boolean): void;
  getPowerState(): Promise<PowerState>;
  readonly onPowerChange: CodegenTypes.EventEmitter<PowerState>;
  readonly onBackgroundTask: CodegenTypes.EventEmitter<BackgroundTaskEvent>;
}

export default TurboModuleRegistry.get<Spec>('RsLifecycle');
