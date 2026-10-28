import type { EventSubscription } from 'react-native';
import NativeRsAndroidSession from '../specs/NativeRsAndroidSession';
import type {
  ActiveSession,
  NotificationAction,
  PipChange,
  TileConnect,
} from '../specs/NativeRsAndroidSession';

export function setActiveSessions(sessions: ActiveSession[]): void {
  NativeRsAndroidSession?.setActiveSessions(sessions);
}

export function setPipEligible(
  eligible: boolean,
  aspectWidth: number,
  aspectHeight: number,
): void {
  NativeRsAndroidSession?.setPipEligible(eligible, aspectWidth, aspectHeight);
}

export function isInPip(): boolean {
  return NativeRsAndroidSession?.isInPip() ?? false;
}

export function setTileTarget(hostId: string, label: string): void {
  NativeRsAndroidSession?.setTileTarget(hostId, label);
}

export function takePendingTileConnect(): string {
  return NativeRsAndroidSession?.takePendingTileConnect() ?? '';
}

function subscribe<T>(
  event: ((listener: (value: T) => void) => unknown) | undefined,
  listener: (value: T) => void,
): () => void {
  if (event === undefined) {
    return () => undefined;
  }
  const subscription = event(listener) as EventSubscription;
  return () => subscription.remove();
}

export function subscribeNotificationAction(
  listener: (event: NotificationAction) => void,
): () => void {
  const module = NativeRsAndroidSession;
  return subscribe(module?.onNotificationAction, listener);
}

export function subscribePipChange(
  listener: (event: PipChange) => void,
): () => void {
  const module = NativeRsAndroidSession;
  return subscribe(module?.onPipChange, listener);
}

export function subscribeTileConnect(
  listener: (event: TileConnect) => void,
): () => void {
  const module = NativeRsAndroidSession;
  return subscribe(module?.onTileConnect, listener);
}
