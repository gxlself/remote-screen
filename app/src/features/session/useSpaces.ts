import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { RefObject } from 'react';
import { Platform } from 'react-native';
import type {
  RemoteSessionHandle,
  SpaceTile,
} from '../../native/session/RemoteSessionView';
import {
  SLIDE_FALLBACK_MS,
  SNAPSHOT_WAIT_MS,
  SPACE_FADE_MS,
  SPACE_HINT_MS,
  clearArrival,
  findScreen,
  peekArrival,
  spaceDirection,
  spacesOf,
} from './spaces';
import type { SwitcherItem, SwitcherList } from './switcher';

export type SpacesOptions = {
  list: SwitcherList;
  /** The session this page shows now. */
  currentId: string | null;
  /** The session the page was opened for (route param): known at once. */
  routeSessionId: string | undefined;
  /** This page's first frame is on screen. */
  framed: boolean;
  videoRef: RefObject<RemoteSessionHandle | null>;
  /** Folds this session away and replaces the page with `item`. */
  switchTo(item: SwitcherItem): void;
};

export type SpaceIndicatorState = {
  highlightId: string;
  fading: boolean;
};

export type SpacesController = {
  /** Open computers in swipe order, for the native view. */
  spaces: SpaceTile[];
  /** Show this session's last picture until its first frame (a switch). */
  showSnapshot: boolean;
  /** The position dots, or null when hidden. */
  indicator: SpaceIndicatorState | null;
  onSpaceSwipe(event: { phase: string }): void;
  onSpaceSwitch(event: { sessionId: string }): void;
  /** ⌃⌥⌘ keys: the same slide as the swipe, then the switch. */
  slideTo(item: SwitcherItem): void;
};

type HintPhase = 'hidden' | 'shown' | 'fading';

/**
 * the page side of switching computers by swiping: which
 * computers sit left and right, when the dots show, and the commit order
 * (slide → fold away → replace → last picture → first frame).
 */
export function useSpaces(options: SpacesOptions): SpacesController {
  const { list, currentId, routeSessionId, framed } = options;
  const latest = useRef(options);
  latest.current = options;
  const spaces = useMemo(() => spacesOf(list), [list]);

  // Switched to: open on the last picture instead of "connecting".
  const [arrived] = useState(
    () => routeSessionId !== undefined && peekArrival(routeSessionId),
  );
  const [waitedOut, setWaitedOut] = useState(false);
  useEffect(() => {
    if (!arrived || routeSessionId === undefined) {
      return;
    }
    clearArrival(routeSessionId);
    const timer = setTimeout(() => setWaitedOut(true), SNAPSHOT_WAIT_MS);
    return () => clearTimeout(timer);
  }, [arrived, routeSessionId]);
  // Only the iOS view keeps last pictures; Android shows its usual wait.
  const showSnapshot =
    Platform.OS === 'ios' && arrived && !framed && !waitedOut;

  const [hint, setHint] = useState<HintPhase>(arrived ? 'shown' : 'hidden');
  const hintTimers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const clearHintTimers = useCallback(() => {
    for (const timer of hintTimers.current) {
      clearTimeout(timer);
    }
    hintTimers.current = [];
  }, []);
  const fadeLater = useCallback(() => {
    clearHintTimers();
    hintTimers.current = [
      setTimeout(() => setHint('fading'), SPACE_HINT_MS),
      setTimeout(() => setHint('hidden'), SPACE_HINT_MS + SPACE_FADE_MS),
    ];
  }, [clearHintTimers]);
  const holdHint = () => {
    clearHintTimers();
    setHint('shown');
  };
  useEffect(() => {
    if (arrived) {
      fadeLater();
    }
    return clearHintTimers;
  }, [arrived, clearHintTimers, fadeLater]);

  const switching = useRef(false);
  const fallback = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (fallback.current !== null) {
        clearTimeout(fallback.current);
      }
    },
    [],
  );
  const commit = (item: SwitcherItem) => {
    if (fallback.current !== null) {
      clearTimeout(fallback.current);
      fallback.current = null;
    }
    if (switching.current || item.id === latest.current.currentId) {
      return;
    }
    switching.current = true;
    latest.current.switchTo(item);
  };

  const onSpaceSwipe = ({ phase }: { phase: string }) => {
    if (phase === 'began') {
      holdHint();
    } else {
      fadeLater();
    }
  };
  const onSpaceSwitch = ({ sessionId }: { sessionId: string }) => {
    const item = findScreen(latest.current.list, sessionId);
    if (item === undefined) {
      // Ended meanwhile: the native view slides back on its own.
      fadeLater();
      return;
    }
    commit(item);
  };
  const slideTo = (item: SwitcherItem) => {
    const { currentId: from, videoRef } = latest.current;
    const direction = spaceDirection(spaces, from, item.id);
    const handle = videoRef.current;
    if (direction === null || handle === null || switching.current) {
      commit(item);
      return;
    }
    holdHint();
    handle.animateSpace(item.id, direction);
    // The native view asks for the switch when the slide ends; if it never
    // does (not on screen), switch anyway.
    if (!switching.current) {
      fallback.current = setTimeout(() => {
        fallback.current = null;
        commit(item);
      }, SLIDE_FALLBACK_MS);
    }
  };

  const highlightId = currentId ?? routeSessionId ?? null;
  const indicator =
    hint === 'hidden' ||
    highlightId === null ||
    !spaces.some(space => space.id === highlightId)
      ? null
      : { highlightId, fading: hint === 'fading' };

  return {
    spaces,
    showSnapshot,
    indicator,
    onSpaceSwipe,
    onSpaceSwitch,
    slideTo,
  };
}
