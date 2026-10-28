import type { KeyboardEvent } from 'react';

import styles from './RadioGroup.module.css';

export interface RadioGroupOption<T extends string> {
  value: T;
  title: string;
  description?: string;
  disabled?: boolean;
}

export interface RadioGroupProps<T extends string> {
  value: T;
  onChange(value: T): void;
  options: Array<RadioGroupOption<T>>;
  label?: string;
  labelledBy?: string;
  boxed?: boolean;
}

function findNextEnabledIndex<T extends string>(
  options: Array<RadioGroupOption<T>>,
  currentIndex: number,
  step: -1 | 1,
): number {
  for (let offset = 1; offset <= options.length; offset += 1) {
    const index = (currentIndex + step * offset + options.length) % options.length;
    if (!options[index]?.disabled) {
      return index;
    }
  }

  return -1;
}

export function RadioGroup<T extends string>({
  value,
  onChange,
  options,
  label,
  labelledBy,
  boxed = false,
}: RadioGroupProps<T>) {
  const selectedIndex = options.findIndex((option) => option.value === value);
  const tabbableIndex =
    selectedIndex >= 0 && !options[selectedIndex]?.disabled
      ? selectedIndex
      : options.findIndex((option) => !option.disabled);

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, currentIndex: number) => {
    const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
    if (step === 0) {
      return;
    }

    event.preventDefault();
    const nextIndex = findNextEnabledIndex(options, currentIndex, step);
    const nextOption = options[nextIndex];
    if (!nextOption) {
      return;
    }

    onChange(nextOption.value);
    const radios =
      event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]');
    radios?.[nextIndex]?.focus();
  };

  return (
    <div
      aria-label={labelledBy ? undefined : label}
      aria-labelledby={labelledBy}
      className={[styles.group, boxed ? styles.boxed : undefined].filter(Boolean).join(' ')}
      role="radiogroup"
    >
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            aria-checked={selected}
            className={[styles.option, boxed ? styles.boxedOption : undefined]
              .filter(Boolean)
              .join(' ')}
            disabled={option.disabled}
            key={option.value}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => handleKeyDown(event, index)}
            role="radio"
            tabIndex={index === tabbableIndex ? 0 : -1}
            type="button"
          >
            <span
              aria-hidden="true"
              className={[styles.indicator, selected ? styles.indicatorSelected : undefined]
                .filter(Boolean)
                .join(' ')}
            >
              {selected ? <span className={styles.dot} /> : null}
            </span>
            <span className={styles.copy}>
              <span className={styles.title}>{option.title}</span>
              {option.description !== undefined ? (
                <span className={styles.description}>{option.description}</span>
              ) : null}
            </span>
          </button>
        );
      })}
    </div>
  );
}
