import { create } from 'zustand';
import type { PickedFile } from './filesApi';
import { classifyPairLink } from '../pairing/pairLinks';

const FILE_URL = /^file:\/\//i;

export type IncomingShareState = {
  pending: PickedFile[];
  push(file: PickedFile): void;
  take(): PickedFile[];
};

/** Returns true for files opened through the iOS document association. */
export function isIncomingFileUrl(url: string): boolean {
  return FILE_URL.test(url) && classifyPairLink(url) === null;
}

export const useIncomingShare = create<IncomingShareState>((set, get) => ({
  pending: [],
  push: file => {
    set(state => ({ pending: [...state.pending, file] }));
  },
  take: () => {
    const pending = get().pending;
    if (pending.length === 0) {
      return [];
    }
    set({ pending: [] });
    return pending;
  },
}));

// Non-React consumers (the URL listener and tests) can use the same store
// through a descriptive alias.
export const incomingShare = Object.assign(useIncomingShare, {
  push: (file: PickedFile) => useIncomingShare.getState().push(file),
  take: () => useIncomingShare.getState().take(),
});
