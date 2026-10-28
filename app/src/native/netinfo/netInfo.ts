import type { EventSubscription } from 'react-native';
import NativeNetInfoPlus from '../specs/NativeNetInfoPlus';
import type { NativeNetStatus } from '../specs/NativeNetInfoPlus';

export type NativeNetworkListener = (status: NativeNetStatus) => void;

export function getNativeNetwork(): Promise<NativeNetStatus | null> {
  const module = NativeNetInfoPlus;
  if (module == null) {
    return Promise.resolve(null);
  }
  return module.getStatus();
}

export function subscribeNativeNetwork(
  listener: NativeNetworkListener,
): () => void {
  const module = NativeNetInfoPlus;
  if (module == null) {
    return () => undefined;
  }
  const subscription = module.onChange(listener) as EventSubscription;
  return () => subscription.remove();
}

export function setVpnPools(pools: string[]): void {
  NativeNetInfoPlus?.setVpnPools(pools);
}
