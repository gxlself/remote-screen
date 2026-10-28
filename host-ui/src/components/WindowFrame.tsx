import type { ReactNode } from 'react';

import styles from './WindowFrame.module.css';

export interface WindowFrameProps {
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  titleId?: string;
}

export function WindowFrame({ title, children, footer, titleId }: WindowFrameProps) {
  return (
    <section aria-labelledby={titleId} className={styles.window}>
      <header className={styles.titleBar} data-tauri-drag-region>
        <h2 className={styles.title} id={titleId}>
          {title}
        </h2>
      </header>
      <div className={styles.content}>{children}</div>
      {footer !== undefined && footer !== null ? (
        <footer className={styles.footer}>{footer}</footer>
      ) : null}
    </section>
  );
}
