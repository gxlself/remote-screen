import type { SpaceTile } from '../../native/session/RemoteSessionView';
import type { SwitcherItem, SwitcherList } from './switcher';

/**
 * the computers that are connected, side by side like the spaces
 * on a Mac. Three fingers left or right move between them; ⌃⌥⌘←/→ and the
 * switcher go the same way.
 */

/** The position dots stay this long after a switch, then fade. */
export const SPACE_HINT_MS = 1500;
export const SPACE_FADE_MS = 250;
/** A switched-to page shows the last picture at most this long. */
export const SNAPSHOT_WAIT_MS = 3000;
/** The keyboard slide normally ends well within this; then switch anyway. */
export const SLIDE_FALLBACK_MS = 800;
/** How long a switch counts as "arriving" for the page it opens. */
const ARRIVAL_MS = 10_000;

export type SpaceDirection = 'next' | 'previous';

/** Full-screen sessions only, in the switcher's order; terminals stay out. */
export function spacesOf(list: SwitcherList): SpaceTile[] {
  return list.screens.map(item => ({ id: item.id, name: item.name }));
}

/** Which way `targetId` lies from `currentId`; null when not both present. */
export function spaceDirection(
  spaces: readonly { id: string }[],
  currentId: string | null,
  targetId: string,
): SpaceDirection | null {
  const from = spaces.findIndex(space => space.id === currentId);
  const to = spaces.findIndex(space => space.id === targetId);
  if (from < 0 || to < 0 || from === to) {
    return null;
  }
  return to > from ? 'next' : 'previous';
}

/** Where `id` sits, for the dots: index and count, or null. */
export function spacePosition(
  spaces: readonly { id: string }[],
  id: string | null,
): { index: number; count: number } | null {
  const index = spaces.findIndex(space => space.id === id);
  return index < 0 ? null : { index, count: spaces.length };
}

export function findScreen(
  list: SwitcherList,
  id: string,
): SwitcherItem | undefined {
  return list.screens.find(item => item.id === id);
}

// The page a switch opens learns that it was switched to (and shows the last
// picture and the dots) without a route param.
let arrival: { id: string; atMs: number } | null = null;

export function markArrival(id: string, nowMs: number = Date.now()): void {
  arrival = { id, atMs: nowMs };
}

/** Whether the page for `id` was just switched to. Does not consume it. */
export function peekArrival(id: string, nowMs: number = Date.now()): boolean {
  return (
    arrival !== null &&
    arrival.id === id &&
    nowMs >= arrival.atMs &&
    nowMs - arrival.atMs < ARRIVAL_MS
  );
}

export function clearArrival(id: string): void {
  if (arrival?.id === id) {
    arrival = null;
  }
}

export function resetArrival(): void {
  arrival = null;
}
