import type { JSX } from 'react';

import { Banner, Button } from '@host-ui/components';

import styles from './StartupScreen.module.css';
import { text } from './text';
import type { StartupState } from './useStartup';

export type StartupScreenProps = {
  state: Exclude<StartupState, { phase: 'ready' }>;
  onRetry: () => void;
};

export function StartupScreen({ state, onRetry }: StartupScreenProps): JSX.Element {
  if (state.phase !== 'failed') {
    return (
      <main aria-busy="true" className={styles.screen}>
        <p className={styles.status} role="status">
          {state.phase === 'starting' ? text.starting : text.registering}
        </p>
      </main>
    );
  }
  return (
    <main className={styles.screen}>
      <div className={styles.panel}>
        <Banner role="alert" title={text.failed} tone="bad">
          {state.message}
        </Banner>
        <div className={styles.actions}>
          <Button onClick={onRetry}>{text.retry}</Button>
        </div>
      </div>
    </main>
  );
}
