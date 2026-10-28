import { useStore } from 'zustand';
import { createStore } from 'zustand/vanilla';

export type PointerLockReason =
  | 'noMouse'
  | 'notStreaming'
  | 'hidden'
  | 'escaped'
  | 'locked'
  | 'notFullScreen'
  | 'pending';

export type PointerLockState = {
  mouse: boolean;
  locked: boolean;
  reason: PointerLockReason;
};

type PointerLockStoreState = {
  states: Record<string, PointerLockState>;
};

export const pointerLockStore = createStore<PointerLockStoreState>(() => ({
  states: {},
}));

function normalizeReason(reason: string): PointerLockReason {
  switch (reason) {
    case 'noMouse':
    case 'notStreaming':
    case 'hidden':
    case 'escaped':
    case 'locked':
    case 'notFullScreen':
    case 'pending':
      return reason;
    default:
      return 'pending';
  }
}

export function normalizePointerLock(raw: {
  mouse: boolean;
  locked: boolean;
  reason: string;
}): PointerLockState {
  return {
    mouse: Boolean(raw.mouse),
    locked: Boolean(raw.locked),
    reason: normalizeReason(raw.reason),
  };
}

export function setPointerLock(id: string, state: PointerLockState): void {
  pointerLockStore.setState(current => {
    const previous = current.states[id];
    if (
      previous !== undefined &&
      previous.mouse === state.mouse &&
      previous.locked === state.locked &&
      previous.reason === state.reason
    ) {
      return current;
    }
    return { states: { ...current.states, [id]: state } };
  });
}

export function clearPointerLock(id: string): void {
  pointerLockStore.setState(current => {
    if (current.states[id] === undefined) {
      return current;
    }
    const states = { ...current.states };
    delete states[id];
    return { states };
  });
}

export function usePointerLock(id: string | null): PointerLockState | null {
  return useStore(pointerLockStore, state =>
    id === null ? null : (state.states[id] ?? null),
  );
}

export type PointerChipKind = 'split' | 'escaped';

export function pointerChipKind(
  state: PointerLockState | null,
  isPad: boolean,
): PointerChipKind | null {
  if (!isPad || state === null || !state.mouse || state.locked) {
    return null;
  }
  if (state.reason === 'notFullScreen') {
    return 'split';
  }
  if (state.reason === 'escaped') {
    return 'escaped';
  }
  return null;
}
