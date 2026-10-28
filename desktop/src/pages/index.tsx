import { useCallback, useState, type JSX } from 'react';

import { ComputersPage } from './computers/ComputersPage';
import type { ComputerRow } from './computers/model';
import { PairPage } from './pair/PairPage';
import styles from './Pages.module.css';
import { SessionNotices } from './session/SessionNotices';

type Route = { page: 'list' } | { page: 'pair' };

/** The main window after startup: computers and pairing. */
export function Pages(): JSX.Element {
  const [route, setRoute] = useState<Route>({ page: 'list' });
  const [hostNames, setHostNames] = useState<ReadonlyMap<string, string>>(new Map());

  const toList = useCallback(() => setRoute({ page: 'list' }), []);
  const toPair = useCallback(() => setRoute({ page: 'pair' }), []);
  const rememberNames = useCallback((rows: ComputerRow[]) => {
    setHostNames(
      (current) => new Map([...current, ...rows.map((r) => [r.hostId, r.name] as const)]),
    );
  }, []);

  return (
    <main className={styles.pages}>
      <SessionNotices hostNames={hostNames} />
      {route.page === 'pair' ? (
        <PairPage onBack={toList} />
      ) : (
        <ComputersPage onPair={toPair} onRows={rememberNames} />
      )}
    </main>
  );
}
