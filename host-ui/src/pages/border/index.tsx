import type { JSX } from 'react';

import type { PageProps } from '../../app/router';

import styles from './border.module.css';

export default function BorderPage(_props: PageProps): JSX.Element {
  return (
    <main data-page="border">
      <div aria-hidden="true" className={styles.border} data-testid="controlled-border" />
    </main>
  );
}
