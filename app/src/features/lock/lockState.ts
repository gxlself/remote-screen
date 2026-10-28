import { create } from 'zustand';

export type LockState = {
  locked: boolean;
  setLocked: (locked: boolean) => void;
};

export const useLockState = create<LockState>(set => ({
  locked: false,
  setLocked: locked => set({ locked }),
}));

// A named store alias is useful to non-React integration code and keeps the
// state source explicit for deep-link handling.
export const lockState = useLockState;
