import { create } from 'zustand';
import type { NativeNetStatus } from '../specs/NativeNetInfoPlus';
import type { NetworkStatus } from '../rscore/types';

export type NativeNetworkState = {
  native: NativeNetStatus | null;
  core: NetworkStatus | null;
  setNative: (native: NativeNetStatus | null) => void;
  setCore: (core: NetworkStatus | null) => void;
};

export const useNativeNetwork = create<NativeNetworkState>(set => ({
  native: null,
  core: null,
  setNative: native => set({ native }),
  setCore: core => set({ core }),
}));

// A named store export makes imperative updates and test inspection explicit.
export const nativeNetworkStore = useNativeNetwork;
