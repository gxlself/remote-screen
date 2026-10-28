import type { ReactNode } from 'react';

import styles from './Banner.module.css';

export type BannerTone = 'controlled' | 'good' | 'bad' | 'info';

export interface BannerProps {
  tone: BannerTone;
  icon?: ReactNode;
  title?: ReactNode;
  children?: ReactNode;
  role?: 'status' | 'alert';
}

const toneClasses: Record<BannerTone, string | undefined> = {
  controlled: styles.controlled,
  good: styles.good,
  bad: styles.bad,
  info: styles.info,
};

export function Banner({ tone, icon, title, children, role }: BannerProps) {
  return (
    <div className={[styles.banner, toneClasses[tone]].join(' ')} role={role}>
      {icon !== undefined && icon !== null ? (
        <span aria-hidden="true" className={styles.icon}>
          {icon}
        </span>
      ) : null}
      <div className={styles.content}>
        {title !== undefined && title !== null ? <div className={styles.title}>{title}</div> : null}
        {children !== undefined && children !== null ? (
          <div className={styles.body}>{children}</div>
        ) : null}
      </div>
    </div>
  );
}
