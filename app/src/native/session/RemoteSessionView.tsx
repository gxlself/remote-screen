import { useEffect, useImperativeHandle, useMemo, useRef } from 'react';
import type React from 'react';
import { StyleSheet } from 'react-native';
import NativeRemoteSessionView, {
  Commands,
} from '../specs/RemoteSessionViewNativeComponent';
import type { VideoStats } from '../specs/RemoteSessionViewNativeComponent';
import {
  clearPointerLock,
  normalizePointerLock,
  setPointerLock,
} from './pointerLockStore';
import type { PointerLockState } from './pointerLockStore';

export type RemoteVideoStats = VideoStats;

export type RemoteSessionHandle = {
  zoomReset(): void;
  centerCursor(): void;
  showMagnifier(on: boolean): void;
  /** slide to `sessionId`, then onSpaceSwitch fires. */
  animateSpace(sessionId: string, direction: 'next' | 'previous'): void;
};

/** One open computer in the three-finger swipe order. */
export type SpaceTile = { id: string; name: string };

export type RemoteSessionViewProps = {
  sessionId: string | null;
  mode: 'trackpad' | 'touch';
  reduceMotion: boolean;
  toolbarCollapsed: boolean;
  keyboardInset: number;
  cursorSpeed?: number;
  naturalScroll?: boolean;
  haptics?: boolean;
  /** open computers in swipe order (this one included). */
  spaces?: readonly SpaceTile[];
  /** Show this session's last picture until the first frame. */
  showSnapshot?: boolean;
  accessibilityLabel: string;
  onGesture?: (e: { kind: string }) => void;
  onFirstFrame?: () => void;
  onViewport?: (v: { zoom: number; x: number; y: number }) => void;
  onVideoStats?: (s: RemoteVideoStats) => void;
  onPointerLock?: (s: PointerLockState) => void;
  onSpaceSwipe?: (e: { phase: string }) => void;
  onSpaceSwitch?: (e: { sessionId: string }) => void;
  ref?: React.Ref<RemoteSessionHandle>;
};

export function RemoteSessionView(
  p: RemoteSessionViewProps,
): React.JSX.Element {
  const nativeRef =
    useRef<React.ElementRef<typeof NativeRemoteSessionView>>(null);
  useImperativeHandle(
    p.ref,
    () => ({
      zoomReset() {
        if (nativeRef.current) {
          Commands.zoomReset(nativeRef.current);
        }
      },
      centerCursor() {
        if (nativeRef.current) {
          Commands.centerCursor(nativeRef.current);
        }
      },
      showMagnifier(on) {
        if (nativeRef.current) {
          Commands.showMagnifier(nativeRef.current, on);
        }
      },
      animateSpace(sessionId, direction) {
        if (nativeRef.current) {
          Commands.animateSpace(nativeRef.current, sessionId, direction);
        }
      },
    }),
    [],
  );
  const sessionId = p.sessionId;
  const spaces = p.spaces;
  const spaceIds = useMemo(
    () => (spaces ?? []).map(space => space.id),
    [spaces],
  );
  const spaceNames = useMemo(
    () => (spaces ?? []).map(space => space.name),
    [spaces],
  );

  useEffect(
    () => () => {
      if (sessionId !== null) {
        clearPointerLock(sessionId);
      }
    },
    [sessionId],
  );

  return (
    <NativeRemoteSessionView
      ref={nativeRef}
      style={styles.surface}
      accessible
      accessibilityRole="image"
      accessibilityLabel={p.accessibilityLabel}
      sessionId={p.sessionId ?? ''}
      mode={p.mode}
      reduceMotion={p.reduceMotion}
      toolbarCollapsed={p.toolbarCollapsed}
      keyboardInset={p.keyboardInset}
      cursorSpeed={p.cursorSpeed}
      naturalScroll={p.naturalScroll}
      haptics={p.haptics}
      spaceIds={spaceIds}
      spaceNames={spaceNames}
      showSnapshot={p.showSnapshot ?? false}
      onSpaceSwipe={e => p.onSpaceSwipe?.(e.nativeEvent)}
      onSpaceSwitch={e => p.onSpaceSwitch?.(e.nativeEvent)}
      onGesture={e => p.onGesture?.(e.nativeEvent)}
      onFirstFrame={() => p.onFirstFrame?.()}
      onViewport={e => p.onViewport?.(e.nativeEvent)}
      onVideoStats={e => p.onVideoStats?.(e.nativeEvent)}
      onPointerLock={e => {
        const state = normalizePointerLock(e.nativeEvent);
        if (p.sessionId !== null) {
          setPointerLock(p.sessionId, state);
        }
        p.onPointerLock?.(state);
      }}
    />
  );
}

const styles = StyleSheet.create({
  surface: { ...StyleSheet.absoluteFill, backgroundColor: '#000000' },
});
