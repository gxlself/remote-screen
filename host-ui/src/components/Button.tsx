import type { MouseEventHandler, ReactNode } from 'react';

import styles from './Button.module.css';

export type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'ghost';
export type ButtonSize = 'md' | 'sm';

export interface ButtonProps {
  variant?: ButtonVariant;
  size?: ButtonSize;
  disabled?: boolean;
  onClick?: MouseEventHandler<HTMLButtonElement>;
  children: ReactNode;
  'aria-label'?: string;
  autoFocus?: boolean;
}

const variantClasses: Record<ButtonVariant, string | undefined> = {
  primary: styles.primary,
  secondary: styles.secondary,
  danger: styles.danger,
  ghost: styles.ghost,
};

export function Button({
  variant = 'primary',
  size = 'md',
  disabled = false,
  onClick,
  children,
  'aria-label': ariaLabel,
  autoFocus = false,
}: ButtonProps) {
  return (
    <button
      aria-label={ariaLabel}
      autoFocus={autoFocus}
      className={[styles.button, styles[size], variantClasses[variant]].join(' ')}
      disabled={disabled}
      onClick={onClick}
      type="button"
    >
      {children}
    </button>
  );
}
