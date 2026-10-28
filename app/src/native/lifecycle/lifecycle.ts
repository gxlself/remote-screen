import type { EventSubscription } from 'react-native';
import NativeRsLifecycle from '../specs/NativeRsLifecycle';
import type {
  BackgroundTaskEvent,
  PowerState,
} from '../specs/NativeRsLifecycle';

export const DEFAULT_POWER_STATE: PowerState = {
  lowPower: false,
  lowPowerMode: false,
  batteryLevel: -1,
  charging: false,
};

export function previousRunEndedInBackground(): boolean {
  return NativeRsLifecycle?.getConstants().previousRunEndedInBackground ?? false;
}

export function setIdleTimerDisabled(disabled: boolean): void {
  NativeRsLifecycle?.setIdleTimerDisabled(disabled);
}

export function setBackgroundKeepAlive(enabled: boolean): void {
  NativeRsLifecycle?.setBackgroundKeepAlive(enabled);
}

export function getPowerState(): Promise<PowerState> {
  return NativeRsLifecycle?.getPowerState() ?? Promise.resolve(DEFAULT_POWER_STATE);
}

export function subscribePowerChange(listener: (state: PowerState) => void): () => void {
  const module = NativeRsLifecycle;
  if (module == null) {
    return () => undefined;
  }
  const subscription = module.onPowerChange(listener) as EventSubscription;
  return () => subscription.remove();
}

export function subscribeBackgroundTask(
  listener: (event: BackgroundTaskEvent) => void,
): () => void {
  const module = NativeRsLifecycle;
  if (module == null) {
    return () => undefined;
  }
  const subscription = module.onBackgroundTask(listener) as EventSubscription;
  return () => subscription.remove();
}
