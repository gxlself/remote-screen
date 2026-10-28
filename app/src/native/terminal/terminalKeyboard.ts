import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

export type TerminalKeyboardState = {
  visible: boolean;
  overlap: number;
};

type TerminalKeyboardStore = TerminalKeyboardState & {
  setKeyboard: (visible: boolean, overlap: number) => void;
  reset: () => void;
};

const INITIAL: TerminalKeyboardState = { visible: false, overlap: 0 };

const terminalKeyboardStore = create<TerminalKeyboardStore>(set => ({
  ...INITIAL,
  setKeyboard: (visible, overlap) =>
    set({
      visible,
      overlap: Number.isFinite(overlap) && overlap > 0 ? overlap : 0,
    }),
  reset: () => set(INITIAL),
}));

export function useTerminalKeyboard(): TerminalKeyboardState {
  return terminalKeyboardStore(
    useShallow(state => ({
      visible: state.visible,
      overlap: state.overlap,
    })),
  );
}

export function setTerminalKeyboard(visible: boolean, overlap: number): void {
  terminalKeyboardStore.getState().setKeyboard(visible, overlap);
}

export function resetTerminalKeyboard(): void {
  terminalKeyboardStore.getState().reset();
}
