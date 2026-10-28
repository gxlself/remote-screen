import { create } from 'zustand';
import type { PowerState } from '../specs/NativeRsLifecycle';
import { DEFAULT_POWER_STATE } from './lifecycle';

export type PowerStateStore = {
  power: PowerState;
  setPower: (power: PowerState) => void;
};

export const usePowerState = create<PowerStateStore>(set => ({
  power: DEFAULT_POWER_STATE,
  setPower: power => set({ power }),
}));

export const powerStore = usePowerState;
