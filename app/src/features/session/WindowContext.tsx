import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { ReactNode } from 'react';
import { AppState, StyleSheet, View, useWindowDimensions } from 'react-native';
import { windowsSupported } from '../../native/window/window';
import {
  MAIN_WINDOW,
  registerWindowPage,
  releaseWindowPage,
  windows,
} from './windows';
import type { WindowPhase } from './windows';
import type { SwitcherItem } from './switcher';

type Size = { width: number; height: number };

type WindowValue = {
  windowId: string;
  /** The window's own size; React Native's window size is the key window's. */
  size: Size | null;
};

const WindowContext = createContext<WindowValue>({
  windowId: MAIN_WINDOW,
  size: null,
});

/** the system window a React tree is drawn in, and its size. */
export function WindowProvider({
  windowId,
  children,
}: {
  windowId: string;
  children: ReactNode;
}) {
  const [size, setSize] = useState<Size | null>(null);
  const value = useMemo(() => ({ windowId, size }), [size, windowId]);
  return (
    <WindowContext.Provider value={value}>
      <View
        onLayout={event => {
          const { width, height } = event.nativeEvent.layout;
          setSize(previous =>
            previous !== null &&
            previous.width === width &&
            previous.height === height
              ? previous
              : { width, height },
          );
        }}
        style={styles.fill}
        testID={`window-root-${windowId}`}
      >
        {children}
      </View>
    </WindowContext.Provider>
  );
}

export function useWindowId(): string {
  return useContext(WindowContext).windowId;
}

/** This window's size: half or a third of the screen in split view. */
export function useWindowSize(): Size {
  const dims = useWindowDimensions();
  const { size } = useContext(WindowContext);
  return size ?? { width: dims.width, height: dims.height };
}

/** The window's own size is known (its first layout came). */
export function useWindowMeasured(): boolean {
  return useContext(WindowContext).size !== null;
}

/** Below this width the session toolbars use the phone layout. */
export const NARROW_WINDOW = 500;

export function phaseOfAppState(
  state: string | null | undefined,
): WindowPhase {
  if (state === 'active') {
    return 'active';
  }
  return state === 'background' ? 'background' : 'inactive';
}

/**
 * The scene state of `windowId` now. Without system windows (iPhone,
 * Android) a window is the app, so the app state answers.
 */
export function windowPhaseNow(windowId: string): WindowPhase {
  if (!windowsSupported()) {
    return phaseOfAppState(AppState.currentState);
  }
  return windows.getState().phases[windowId] ?? 'active';
}

/** Calls `listener` when the window's scene state changes. */
export function subscribeWindowPhase(
  windowId: string,
  listener: (phase: WindowPhase) => void,
): () => void {
  if (!windowsSupported()) {
    const subscription = AppState.addEventListener('change', state =>
      listener(phaseOfAppState(state)),
    );
    return () => subscription.remove();
  }
  let last = windowPhaseNow(windowId);
  return windows.subscribe(state => {
    const next = state.phases[windowId] ?? 'active';
    if (next !== last) {
      last = next;
      listener(next);
    }
  });
}

/**
 * The session page's view of the foreground: its own window's scene state,
 * not the app's. In split view both windows are on screen.
 */
export function useWindowPhase(): WindowPhase {
  const windowId = useWindowId();
  const [phase, setPhase] = useState(() => windowPhaseNow(windowId));
  useEffect(() => {
    setPhase(windowPhaseNow(windowId));
    return subscribeWindowPhase(windowId, setPhase);
  }, [windowId]);
  return phase;
}

let nextPage = 0;

/**
 * Tells the other windows that this page shows `item` (or nothing while
 * null), so they do not take the session over.
 */
export function useWindowPage(item: SwitcherItem | null): void {
  const windowId = useWindowId();
  const [key] = useState(() => `page-${(nextPage += 1)}`);
  const latest = useRef(item);
  latest.current = item;
  const shownKey =
    item === null ? null : `${item.id}\u0000${item.status}\u0000${item.name}`;
  useEffect(() => {
    const current = latest.current;
    if (shownKey === null || current === null) {
      releaseWindowPage(key);
      return;
    }
    registerWindowPage(key, windowId, current);
  }, [key, shownKey, windowId]);
  useEffect(() => () => releaseWindowPage(key), [key]);
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
});
