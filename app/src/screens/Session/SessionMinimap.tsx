import { useEffect, useRef, useState } from 'react';
import { Animated, Pressable, StyleSheet, View } from 'react-native';
import { useVideoStats, useViewport } from '../../features/session/videoStore';
import { region, visibleRect } from '../../features/session/viewportText';
import { useScreenReader } from '../../features/session/useScreenReader';
import { sessionViewport } from '../../i18n/zh/sessionViewport';
import type { SlotProps } from './slots';

const FRAME_WIDTH = 98;
export const FRAME_HEIGHT = 63;
const DEFAULT_ASPECT = 16 / 10;
const FADE_MS = 140;

function screenSize(stats: { width: number; height: number } | null): {
  width: number;
  height: number;
} {
  const aspect =
    stats !== null &&
    Number.isFinite(stats.width) &&
    stats.width > 0 &&
    Number.isFinite(stats.height) &&
    stats.height > 0
      ? stats.width / stats.height
      : DEFAULT_ASPECT;
  const frameWidth = FRAME_WIDTH - 2;
  const frameHeight = FRAME_HEIGHT - 2;
  if (aspect >= frameWidth / frameHeight) {
    return { width: frameWidth, height: frameWidth / aspect };
  }
  return { width: frameHeight * aspect, height: frameHeight };
}

function displayNumber(ctx: SlotProps['ctx']): number | null {
  const displays = ctx.rec?.displays ?? [];
  if (displays.length <= 1) {
    return null;
  }
  const index = displays.findIndex(item => item.id === ctx.rec?.displayId);
  return (index < 0 ? 0 : index) + 1;
}

/** 小地图是否该出现；指针锁定提示据此让位，避免叠在一起。 */
export function minimapVisible(
  ctx: Pick<SlotProps['ctx'], 'view' | 'keyboardOpen'>,
  zoom: number | null,
): boolean {
  return (
    zoom !== null &&
    zoom > 1.05 &&
    ctx.view.overlay === 'none' &&
    !ctx.keyboardOpen
  );
}

export function SessionMinimap({ ctx }: SlotProps) {
  const viewport = useViewport(ctx.sessionId);
  const stats = useVideoStats(ctx.sessionId);
  const screenReader = useScreenReader();
  const wantsVisible = minimapVisible(ctx, viewport?.zoom ?? null);
  const [rendered, setRendered] = useState(wantsVisible);
  const opacity = useRef(new Animated.Value(wantsVisible ? 1 : 0)).current;

  useEffect(() => {
    if (wantsVisible) {
      setRendered(true);
      if (ctx.reduceMotion) {
        opacity.stopAnimation();
        opacity.setValue(1);
        return undefined;
      }
      opacity.setValue(0);
      const animation = Animated.timing(opacity, {
        toValue: 1,
        duration: FADE_MS,
        useNativeDriver: true,
      });
      animation.start();
      return () => animation.stop();
    }
    if (ctx.reduceMotion) {
      opacity.stopAnimation();
      opacity.setValue(0);
      setRendered(false);
      return undefined;
    }
    const animation = Animated.timing(opacity, {
      toValue: 0,
      duration: FADE_MS,
      useNativeDriver: true,
    });
    animation.start(({ finished }) => {
      if (finished) {
        setRendered(false);
      }
    });
    return () => animation.stop();
  }, [ctx.reduceMotion, opacity, wantsVisible]);

  if (screenReader || !rendered || viewport === null) {
    return null;
  }

  const rect = visibleRect(viewport);
  const size = screenSize(stats);
  const right = 1 - (rect.left + rect.width);
  const bottom = 1 - (rect.top + rect.height);
  const label = sessionViewport.minimap.label(displayNumber(ctx), region(rect));

  return (
    <Animated.View style={[styles.anchor, { opacity }]}>
      <Pressable
        accessibilityLabel={label}
        accessibilityRole="button"
        onPress={() => ctx.videoRef.current?.zoomReset()}
        style={styles.frame}
        testID="session-minimap"
      >
        <View
          style={[styles.screen, { width: size.width, height: size.height }]}
          testID="session-minimap-screen"
        >
          <View
            pointerEvents="none"
            style={[styles.dim, styles.dimLeft, { width: pct(rect.left) }]}
          />
          <View
            pointerEvents="none"
            style={[styles.dim, styles.dimRight, { width: pct(right) }]}
          />
          <View
            pointerEvents="none"
            style={[
              styles.dim,
              styles.dimTop,
              {
                left: pct(rect.left),
                right: pct(right),
                height: pct(rect.top),
              },
            ]}
          />
          <View
            pointerEvents="none"
            style={[
              styles.dim,
              styles.dimBottom,
              {
                left: pct(rect.left),
                right: pct(right),
                height: pct(bottom),
              },
            ]}
          />
          <View
            pointerEvents="none"
            style={[
              styles.viewport,
              {
                left: pct(rect.left),
                top: pct(rect.top),
                width: pct(rect.width),
                height: pct(rect.height),
              },
            ]}
            testID="session-minimap-viewport"
          />
        </View>
      </Pressable>
    </Animated.View>
  );
}

function pct(value: number): `${number}%` {
  return `${value * 100}%`;
}

export default SessionMinimap;

const styles = StyleSheet.create({
  anchor: {
    position: 'absolute',
    right: 59,
    top: 12,
    width: FRAME_WIDTH,
    height: FRAME_HEIGHT,
  },
  frame: {
    width: FRAME_WIDTH,
    height: FRAME_HEIGHT,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 8,
    borderWidth: 1,
    borderColor: 'rgba(255,255,255,0.22)',
    backgroundColor: '#0A0D10',
    shadowColor: '#000000',
    shadowOffset: { width: 0, height: 5 },
    shadowOpacity: 0.35,
    shadowRadius: 12,
    elevation: 8,
  },
  screen: {
    overflow: 'hidden',
    borderRadius: 6,
    backgroundColor: '#161B1F',
  },
  dim: {
    position: 'absolute',
    backgroundColor: 'rgba(0,0,0,0.38)',
  },
  dimLeft: { left: 0, top: 0, bottom: 0 },
  dimRight: { right: 0, top: 0, bottom: 0 },
  dimTop: { top: 0 },
  dimBottom: { bottom: 0 },
  viewport: {
    position: 'absolute',
    borderWidth: 1.5,
    borderColor: '#FFFFFF',
  },
});
