import styles from './StatusDot.module.css';

export type StatusTone = 'good' | 'bad' | 'relay' | 'muted';

export interface StatusDotProps {
  tone: StatusTone;
  size?: number;
}

const toneClasses: Record<StatusTone, string | undefined> = {
  good: styles.good,
  bad: styles.bad,
  relay: styles.relay,
  muted: styles.muted,
};

export function StatusDot({ tone, size = 6 }: StatusDotProps) {
  return (
    <span
      aria-hidden="true"
      className={[styles.dot, toneClasses[tone]].join(' ')}
      style={{ width: size, height: size }}
    />
  );
}
