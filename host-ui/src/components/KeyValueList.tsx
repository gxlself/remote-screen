import { Fragment } from 'react';
import type { CSSProperties, ReactNode } from 'react';

import styles from './KeyValueList.module.css';

export interface KeyValueItem {
  key: string;
  value: ReactNode;
  mono?: boolean;
}

export interface KeyValueListProps {
  items: Array<KeyValueItem>;
  keyWidth?: number;
}

export function KeyValueList({ items, keyWidth = 88 }: KeyValueListProps) {
  const style: CSSProperties = { gridTemplateColumns: `${keyWidth}px minmax(0, 1fr)` };
  const visibleItems = items.filter((item) => item.value !== null && item.value !== undefined);

  return (
    <dl className={styles.list} style={style}>
      {visibleItems.map((item, index) => (
        <Fragment key={`${item.key}-${index}`}>
          <dt className={styles.key}>{item.key}</dt>
          <dd
            className={[styles.value, item.mono ? styles.mono : undefined]
              .filter(Boolean)
              .join(' ')}
          >
            {item.value}
          </dd>
        </Fragment>
      ))}
    </dl>
  );
}
