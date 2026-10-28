import { useEffect, useRef, useState } from 'react';
import { PixelRatio, StyleSheet, View } from 'react-native';
import Svg, { Circle } from 'react-native-svg';
import { common } from '../i18n/zh/common';
import { tokens } from '../theme/tokens';
import AppText from './AppText';

const TRACK_COLOR = 'rgba(125,168,255,0.25)';
const STROKE_WIDTH = 2;

export type UseCountdownOptions = {
  running: boolean;
  onDone?: () => void;
  key?: unknown;
};

export function useCountdown(
  totalSec: number,
  { running, onDone, key }: UseCountdownOptions,
): number {
  const [remaining, setRemaining] = useState(totalSec);
  const remainingRef = useRef(totalSec);
  const doneRef = useRef(false);
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  useEffect(() => {
    remainingRef.current = totalSec;
    doneRef.current = false;
    setRemaining(totalSec);
  }, [key, totalSec]);

  useEffect(() => {
    if (!running) {
      return undefined;
    }

    if (remainingRef.current <= 0) {
      if (!doneRef.current) {
        doneRef.current = true;
        onDoneRef.current?.();
      }
      return undefined;
    }

    const interval = setInterval(() => {
      const next = Math.max(0, remainingRef.current - 1);
      remainingRef.current = next;
      setRemaining(next);
      if (next === 0) {
        clearInterval(interval);
        if (!doneRef.current) {
          doneRef.current = true;
          onDoneRef.current?.();
        }
      }
    }, 1000);

    return () => {
      clearInterval(interval);
    };
  }, [key, running, totalSec]);

  return remaining;
}

export type CountdownProps = {
  remaining: number;
  total: number;
  size?: number;
};

export function Countdown({ remaining, total, size = 36 }: CountdownProps) {
  const center = size / 2;
  const radius = Math.max(0, (size - STROKE_WIDTH) / 2);
  const circumference = 2 * Math.PI * radius;
  const ratio = total > 0 ? Math.min(1, Math.max(0, remaining / total)) : 0;
  const progressLength = circumference * ratio;
  const largeFont = PixelRatio.getFontScale() > 2;

  const ring = (
    <Svg
      accessible={false}
      height={size}
      style={StyleSheet.absoluteFill}
      width={size}
    >
      <Circle
        cx={center}
        cy={center}
        fill="none"
        r={radius}
        stroke={TRACK_COLOR}
        strokeWidth={STROKE_WIDTH}
      />
      <Circle
        cx={center}
        cy={center}
        fill="none"
        origin={`${center}, ${center}`}
        r={radius}
        rotation={-90}
        stroke={tokens.accent}
        strokeDasharray={`${progressLength} ${circumference}`}
        strokeLinecap="round"
        strokeWidth={STROKE_WIDTH}
      />
    </Svg>
  );

  return (
    <View
      accessible
      accessibilityLabel={common.a11y.countdown(remaining)}
      accessibilityRole="timer"
      style={[
        styles.container,
        largeFont ? styles.largeContainer : { width: size, height: size },
      ]}
    >
      {largeFont ? (
        <>
          <View style={[styles.ring, { width: size, height: size }]}>
            {ring}
          </View>
          <AppText mono variant="mono" weight={600}>
            {`${remaining} ${common.units.seconds}`}
          </AppText>
        </>
      ) : (
        <>
          {ring}
          <AppText mono variant="mono" weight={600}>
            {remaining}
          </AppText>
          <AppText color="text3" variant="tiny">
            {common.units.seconds}
          </AppText>
        </>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  largeContainer: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  ring: {
    flexShrink: 0,
  },
});
