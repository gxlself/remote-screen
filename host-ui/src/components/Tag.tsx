import type { ReactNode } from 'react';

import styles from './Tag.module.css';

export type TagTone = 'controlling' | 'neutral' | 'accent' | 'relay' | 'good' | 'bad';

export interface TagProps {
  tone: TagTone;
  children: ReactNode;
}

const toneClasses: Record<TagTone, string | undefined> = {
  controlling: styles.controlling,
  neutral: styles.neutral,
  accent: styles.accent,
  relay: styles.relay,
  good: styles.good,
  bad: styles.bad,
};

export function Tag({ tone, children }: TagProps) {
  return <span className={[styles.tag, toneClasses[tone]].join(' ')}>{children}</span>;
}
