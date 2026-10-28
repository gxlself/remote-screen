import { create } from 'zustand';
import type { StoreApi, UseBoundStore } from 'zustand';
import type { TailnetMode } from '../settings/schema';

export type SwitchState = {
  previousMode: TailnetMode | null;
  loginWindowUntilMs: number | null;
  openedAuthUrl: string | null;
};

const initialState: SwitchState = {
  previousMode: null,
  loginWindowUntilMs: null,
  openedAuthUrl: null,
};

export const tailnetSwitchStore: UseBoundStore<StoreApi<SwitchState>> =
  create<SwitchState>(() => ({ ...initialState }));

export const LOGIN_WINDOW_MS = 20_000;

export function resetTailnetSwitchStore(): void {
  tailnetSwitchStore.setState({ ...initialState });
}
