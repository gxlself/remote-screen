import { create, useStore } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import type { AdapterTarget } from '../../native/rscore/types';
import {
  activateWindow,
  closeWindow,
  openWindow,
  windowsSupported,
} from '../../native/window/window';
import { sameTarget } from '../adapters/liveStore';
import type { SwitcherItem } from './switcher';
import { encodeWindowTarget, windowTargetOf } from './windowTarget';

/**
 * on iPad every computer can sit in a system window of its own.
 * One JS runtime renders all windows; this store says which window shows
 * which session and whether each window is on screen.
 */

/** The window with the whole app (computer list, tabs, settings). */
export const MAIN_WINDOW = 'main';

/** A scene's state: inactive still counts as on screen (split view, menus). */
export type WindowPhase = 'active' | 'inactive' | 'background';

export type WindowPage = { windowId: string; item: SwitcherItem };

type PendingWindow = {
  item: SwitcherItem;
  payload: string;
  /** The page showing the session still has to fold it away. */
  waiting: boolean;
};

export type WindowsState = {
  /** Scene state per window; a closed window other than main is absent. */
  phases: Record<string, WindowPhase>;
  /** Session pages, by page key. */
  pages: Record<string, WindowPage>;
  /** Windows asked for whose page is not up yet, by window id. */
  pending: Record<string, PendingWindow>;
};

const EMPTY: WindowsState = { phases: {}, pages: {}, pending: {} };

export const windows = create<WindowsState>(() => EMPTY);

export function resetWindows(): void {
  windows.setState(EMPTY, true);
}

export function isVisiblePhase(phase: WindowPhase | undefined): boolean {
  // Unknown until the first native event: the window that runs JS is up.
  return phase !== 'background';
}

export function windowVisible(state: WindowsState, windowId: string): boolean {
  return isVisiblePhase(state.phases[windowId]);
}

export function anyWindowVisible(state: WindowsState): boolean {
  const phases = Object.values(state.phases);
  return phases.length === 0 || phases.some(isVisiblePhase);
}

/** The window that shows `sessionId` or is being opened for it. */
export function windowShowing(
  state: WindowsState,
  sessionId: string,
): string | null {
  for (const page of Object.values(state.pages)) {
    if (page.item.id === sessionId) {
      return page.windowId;
    }
  }
  for (const [windowId, pending] of Object.entries(state.pending)) {
    if (pending.item.id === sessionId) {
      return windowId;
    }
  }
  return null;
}

/** A window other than `windowId` that shows the session (or will). */
export function otherWindowShowing(
  state: WindowsState,
  windowId: string,
  sessionId: string | undefined,
  target?: AdapterTarget,
): string | null {
  if (sessionId !== undefined) {
    const shown = windowShowing(state, sessionId);
    if (shown !== null && shown !== windowId) {
      return shown;
    }
  }
  if (target === undefined) {
    return null;
  }
  // An adapter page asked for by computer, not by session id.
  const entries = [
    ...Object.values(state.pages),
    ...Object.entries(state.pending).map(([id, pending]) => ({
      windowId: id,
      item: pending.item,
    })),
  ];
  for (const entry of entries) {
    const { item } = entry;
    if (
      entry.windowId !== windowId &&
      item.kind === target.protocol &&
      sameTarget(item.params.target, target)
    ) {
      return entry.windowId;
    }
  }
  return null;
}

/** Sessions on screen in windows other than `windowId`. */
export function shownInOtherWindows(
  state: WindowsState,
  windowId: string,
): SwitcherItem[] {
  const items = new Map<string, SwitcherItem>();
  for (const page of Object.values(state.pages)) {
    if (page.windowId !== windowId) {
      items.set(page.item.id, page.item);
    }
  }
  return [...items.values()];
}

/** Every session a page shows, newest registration last. */
export function shownItems(state: WindowsState): SwitcherItem[] {
  return Object.values(state.pages).map(page => page.item);
}

export function useWindows<T>(selector: (state: WindowsState) => T): T {
  return useStore(windows, useShallow(selector));
}

/** A native scene event; `closed` forgets the window (main only hides). */
export function setWindowPhase(windowId: string, state: string): void {
  windows.setState(current => {
    if (state === 'closed') {
      const phases = { ...current.phases };
      if (windowId === MAIN_WINDOW) {
        phases[windowId] = 'background';
      } else {
        delete phases[windowId];
      }
      const pending = { ...current.pending };
      delete pending[windowId];
      return { ...current, phases, pending };
    }
    if (state !== 'active' && state !== 'inactive' && state !== 'background') {
      return current;
    }
    if (current.phases[windowId] === state) {
      return current;
    }
    return { ...current, phases: { ...current.phases, [windowId]: state } };
  });
}

export function registerWindowPage(
  key: string,
  windowId: string,
  item: SwitcherItem,
): void {
  windows.setState(current => {
    const pending = { ...current.pending };
    if (pending[windowId]?.item.id === item.id) {
      delete pending[windowId];
    }
    return {
      ...current,
      pages: { ...current.pages, [key]: { windowId, item } },
      pending,
    };
  });
}

const OPEN_AFTER_RELEASE_MS = 0;

export function releaseWindowPage(key: string): void {
  const page = windows.getState().pages[key];
  if (page === undefined) {
    return;
  }
  windows.setState(current => {
    const pages = { ...current.pages };
    delete pages[key];
    return { ...current, pages };
  });
  const state = windows.getState();
  if (Object.values(state.pages).some(other => other.item.id === page.item.id)) {
    return;
  }
  for (const [windowId, pending] of Object.entries(state.pending)) {
    if (!pending.waiting || pending.item.id !== page.item.id) {
      continue;
    }
    windows.setState(current => ({
      ...current,
      pending: { ...current.pending, [windowId]: { ...pending, waiting: false } },
    }));
    // The page is gone, its unmount (the fold-away) done in the same commit:
    // the new window picks the session up from where it was handed over.
    setTimeout(
      () => openWindow(windowId, pending.payload, pending.item.name),
      OPEN_AFTER_RELEASE_MS,
    );
  }
}

let nextWindow = 0;

function newWindowId(): string {
  nextWindow += 1;
  return `w${Date.now().toString(36)}${nextWindow}`;
}

export type OpenInWindowResult =
  /** It already has a window, now brought forward. */
  | 'activated'
  /** A new window is on its way. */
  | 'opened'
  /** The caller's page shows it: fold it away, the window opens after. */
  | 'moving'
  | 'unsupported';

/** 在新窗口打开: a window of its own for `item`. */
export function openInNewWindow(
  item: SwitcherItem,
  fromWindowId: string,
): OpenInWindowResult {
  const target = windowTargetOf(item);
  if (target === null || !windowsSupported()) {
    return 'unsupported';
  }
  const state = windows.getState();
  const shown = windowShowing(state, item.id);
  if (shown !== null && shown !== fromWindowId) {
    activateWindow(shown);
    return 'activated';
  }
  const windowId = newWindowId();
  const payload = encodeWindowTarget(target);
  const waiting = shown === fromWindowId;
  windows.setState(current => ({
    ...current,
    pending: { ...current.pending, [windowId]: { item, payload, waiting } },
  }));
  if (!waiting) {
    openWindow(windowId, payload, item.name);
  }
  return waiting ? 'moving' : 'opened';
}

/**
 * A session shown in another window is not taken over: that window comes
 * forward instead. True when it did.
 */
export function focusOtherWindow(
  sessionId: string,
  fromWindowId: string,
): boolean {
  if (!windowsSupported()) {
    return false;
  }
  const shown = windowShowing(windows.getState(), sessionId);
  if (shown === null || shown === fromWindowId) {
    return false;
  }
  activateWindow(shown);
  return true;
}

/** A window of its own is done: close it and show the main window. */
export function leaveWindow(windowId: string): void {
  activateWindow(MAIN_WINDOW);
  if (windowId !== MAIN_WINDOW) {
    closeWindow(windowId);
  }
}

export type PausePlan = {
  pause: SwitcherItem[];
  resume: string[];
  /** No longer shown: forget without a call. */
  drop: string[];
};

/**
 * What to pause and resume so that only sessions in windows off screen are
 * paused. While no window is on screen the whole app is in the background
 * and SessionLifecycle's app-wide rule applies instead. The session in
 * picture in picture keeps playing. A Host session whose page went away
 * resumes (folded-away Host sessions keep running); an adapter page already
 * paused its session as it folded it away. `fresh`: the app just came back
 * from the background, so every shown session is set again.
 */
export function planWindowPauses(
  state: WindowsState,
  paused: ReadonlyMap<string, SwitcherItem['kind']>,
  pipSessionId: string | null,
  fresh = false,
): PausePlan {
  const plan: PausePlan = { pause: [], resume: [], drop: [] };
  if (!anyWindowVisible(state)) {
    return plan;
  }
  const shown = new Map<string, { item: SwitcherItem; visible: boolean }>();
  for (const page of Object.values(state.pages)) {
    const before = shown.get(page.item.id);
    shown.set(page.item.id, {
      item: page.item,
      visible: (before?.visible ?? false) || windowVisible(state, page.windowId),
    });
  }
  for (const [id, { item, visible }] of shown) {
    const want = !visible && id !== pipSessionId;
    if (fresh) {
      if (want) {
        plan.pause.push(item);
      } else {
        plan.resume.push(id);
      }
    } else if (want && !paused.has(id)) {
      plan.pause.push(item);
    } else if (!want && paused.has(id)) {
      plan.resume.push(id);
    }
  }
  for (const [id, kind] of paused) {
    if (shown.has(id)) {
      continue;
    }
    if (kind === 'host') {
      plan.resume.push(id);
    } else {
      plan.drop.push(id);
    }
  }
  return plan;
}

/** A shown session whose window is off screen while another is up. */
export function hiddenByWindow(state: WindowsState, sessionId: string): boolean {
  const pages = Object.values(state.pages).filter(
    page => page.item.id === sessionId,
  );
  return (
    pages.length > 0 &&
    anyWindowVisible(state) &&
    !pages.some(page => windowVisible(state, page.windowId))
  );
}
