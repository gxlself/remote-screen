import styles from './Switch.module.css';

export interface SwitchProps {
  checked: boolean;
  onChange(next: boolean): void;
  labelledBy?: string;
  label?: string;
  disabled?: boolean;
}

export function Switch({ checked, onChange, labelledBy, label, disabled = false }: SwitchProps) {
  return (
    <button
      aria-checked={checked}
      aria-label={labelledBy ? undefined : label}
      aria-labelledby={labelledBy}
      className={[styles.switch, checked ? styles.on : undefined].filter(Boolean).join(' ')}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      role="switch"
      type="button"
    >
      <span
        aria-hidden="true"
        className={[styles.thumb, checked ? styles.thumbOn : undefined].filter(Boolean).join(' ')}
      />
    </button>
  );
}
