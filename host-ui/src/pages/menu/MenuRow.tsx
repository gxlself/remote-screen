import type { ReactNode } from 'react';

import styles from './Menu.module.css';

export interface MenuRowProps {
  children: ReactNode;
  trailing?: ReactNode;
  tone?: 'default' | 'danger' | 'accent';
  disabled?: boolean;
  onClick: () => void;
}

export function MenuRow({
  children,
  trailing,
  tone = 'default',
  disabled = false,
  onClick,
}: MenuRowProps) {
  const toneClass =
    tone === 'danger' ? styles.rowDanger : tone === 'accent' ? styles.rowAccent : undefined;

  return (
    <button
      className={[styles.menuRow, toneClass].filter(Boolean).join(' ')}
      disabled={disabled}
      onClick={onClick}
      type="button"
    >
      <span className={styles.rowLabel}>{children}</span>
      {trailing === undefined || trailing === null ? null : (
        <span className={styles.rowTrailing}>{trailing}</span>
      )}
    </button>
  );
}
