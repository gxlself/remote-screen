import { create } from 'zustand';

export const KEYBAR_HINT_HEIGHT = 44;
export const KEYBAR_HEIGHT = 52;

type KeyboardInsetState = {
  inset: number;
  setInset: (inset: number) => void;
};

const useKeyboardInsetStore = create<KeyboardInsetState>(set => ({
  inset: 0,
  setInset: inset => set({ inset }),
}));

export function useKeyboardInset(): number {
  return useKeyboardInsetStore(state => state.inset);
}

export function setKeyboardInset(inset: number): void {
  useKeyboardInsetStore
    .getState()
    .setInset(Number.isFinite(inset) && inset > 0 ? inset : 0);
}

export function resetKeyboardInset(): void {
  useKeyboardInsetStore.getState().setInset(0);
}
