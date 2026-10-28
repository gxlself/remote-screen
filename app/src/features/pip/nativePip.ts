import { Platform } from 'react-native';
import type { EventSubscription } from 'react-native';
import NativeRsPip from '../../native/specs/NativeRsPip';
import type {
  PipChangeEvent,
  PipPlaybackEvent,
} from '../../native/specs/NativeRsPip';

export type {
  PipChangeEvent,
  PipPlaybackEvent,
} from '../../native/specs/NativeRsPip';

type PipConstants = { supported: boolean };

let constantsRead = false;
let constants: PipConstants | null = null;
let constantsModule: typeof NativeRsPip | null | undefined;

function readConstants(): PipConstants | null {
  const module = NativeRsPip;
  if (!constantsRead || constantsModule !== module) {
    constantsRead = true;
    constantsModule = module;
    constants = module?.getConstants() ?? null;
  }
  return constants;
}

export function pipSupported(): boolean {
  if (Platform.OS !== 'ios') {
    return false;
  }
  return readConstants()?.supported ?? false;
}

export function setPipEligibility(
  sessionId: string | null,
  canStart: boolean,
  canContinue: boolean,
): void {
  NativeRsPip?.setEligibility(sessionId ?? '', canStart, canContinue);
}

export function startPip(): void {
  NativeRsPip?.start();
}

export function stopPip(): void {
  NativeRsPip?.stop();
}

export function isPipActive(): boolean {
  return NativeRsPip?.isActive() ?? false;
}

function subscribe<T>(
  event: ((listener: (value: T) => void) => EventSubscription) | undefined,
  listener: (value: T) => void,
): () => void {
  if (event === undefined) {
    return () => undefined;
  }
  const subscription = event(listener);
  return () => subscription.remove();
}

export function subscribePipChange(
  listener: (event: PipChangeEvent) => void,
): () => void {
  return subscribe(NativeRsPip?.onPipChange, listener);
}

export function subscribePipPlayback(
  listener: (event: PipPlaybackEvent) => void,
): () => void {
  return subscribe(NativeRsPip?.onPipPlayback, listener);
}
