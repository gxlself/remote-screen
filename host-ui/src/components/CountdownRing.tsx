import type { CSSProperties } from 'react';

import styles from './CountdownRing.module.css';

export interface CountdownRingProps {
  total: number;
  remaining: number;
  color?: string;
  size?: number;
}

export function ringOffset(total: number, remaining: number, circumference: number): number {
  const progress = total > 0 ? remaining / total : 0;
  const clamped = Math.min(1, Math.max(0, progress));
  return circumference * (1 - clamped);
}

export function CountdownRing({
  total,
  remaining,
  color = 'var(--rs-hostControlled)',
  size = 16,
}: CountdownRingProps) {
  const radius = size / 2 - 1.5;
  const circumference = 2 * Math.PI * radius;
  const center = size / 2;
  const style = { '--rs-ring-color': color } as CSSProperties;

  return (
    <svg
      aria-hidden="true"
      className={styles.ring}
      focusable="false"
      height={size}
      style={style}
      viewBox={`0 0 ${size} ${size}`}
      width={size}
    >
      <circle
        className={styles.track}
        cx={center}
        cy={center}
        fill="none"
        r={radius}
        strokeWidth="2"
      />
      <circle
        className={styles.progress}
        cx={center}
        cy={center}
        fill="none"
        r={radius}
        strokeDasharray={circumference}
        strokeDashoffset={ringOffset(total, remaining, circumference)}
        strokeWidth="2"
      />
    </svg>
  );
}
