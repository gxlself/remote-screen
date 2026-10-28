import { useSyncExternalStore } from 'react';
import type { PipChangeEvent } from './nativePip';

export type PipState = { active: boolean; sessionId: string | null };

const INITIAL_STATE: PipState = { active: false, sessionId: null };
let state: PipState = INITIAL_STATE;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function publish(next: PipState): void {
  if (state.active === next.active && state.sessionId === next.sessionId) {
    return;
  }
  state = next;
  for (const listener of listeners) {
    listener();
  }
}

export function getPipState(): PipState {
  return state;
}

export function usePipState(): PipState {
  return useSyncExternalStore(subscribe, getPipState, getPipState);
}

export function applyPipChange(event: PipChangeEvent): void {
  publish({
    active: event.active,
    sessionId: event.active ? event.sessionId : null,
  });
}

export function resetPipStateForTests(): void {
  publish(INITIAL_STATE);
}
