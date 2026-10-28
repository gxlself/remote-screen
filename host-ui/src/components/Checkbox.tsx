import type { ReactNode } from 'react';

import styles from './Checkbox.module.css';

export interface CheckboxProps {
  checked: boolean;
  onChange(next: boolean): void;
  label: ReactNode;
  description?: string;
  disabled?: boolean;
}

export function Checkbox({
  checked,
  onChange,
  label,
  description,
  disabled = false,
}: CheckboxProps) {
  return (
    <button
      aria-checked={checked}
      aria-disabled={disabled}
      className={[styles.checkbox, disabled ? styles.disabled : undefined]
        .filter(Boolean)
        .join(' ')}
      disabled={disabled}
      onClick={() => {
        if (!disabled) {
          onChange(!checked);
        }
      }}
      role="checkbox"
      type="button"
    >
      <span
        aria-hidden="true"
        className={[styles.box, checked ? styles.boxChecked : undefined].filter(Boolean).join(' ')}
      >
        {checked ? (
          <svg aria-hidden="true" className={styles.check} height="8" viewBox="0 0 10 8" width="10">
            <path
              d="M1 4.2 3.8 7 9 1"
              fill="none"
              stroke="currentColor"
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth="1.5"
            />
          </svg>
        ) : null}
      </span>
      <span className={styles.copy}>
        <span className={styles.label}>{label}</span>
        {description !== undefined ? (
          <span className={styles.description}>{description}</span>
        ) : null}
      </span>
    </button>
  );
}
