import { useEffect, useState } from 'react';
import { create } from 'zustand';
import type { SessionId } from '../../native/rscore/types';
import { useAdapterLive } from '../adapters/liveStore';
import type { AdapterLiveState } from '../adapters/liveStore';
import { selectLiveSsh, useSshSessions } from '../sessions/sshStore';
import { selectLive, useSessions } from './store';

/**
 * Screen Sharing / Remote Desktop pages that are connecting or connected,
 * keyed per page. The id is null until the adapter hands one out.
 */
type AdapterPages = Record<string, SessionId | null>;

const adapterPages = create<AdapterPages>(() => ({}));
let nextPage = 0;

/** Counts this adapter page as a live session while `live` holds. */
export function useAdapterPageLive(
  sessionId: SessionId | null,
  live: boolean,
): void {
  const [key] = useState(() => `adapter-page-${(nextPage += 1)}`);
  useEffect(() => {
    if (!live) {
      return;
    }
    adapterPages.setState({ [key]: sessionId });
    return () => {
      adapterPages.setState(
        state =>
          Object.fromEntries(
            Object.entries(state).filter(([page]) => page !== key),
          ),
        true,
      );
    };
  }, [key, live, sessionId]);
}

export function resetAdapterPages(): void {
  adapterPages.setState({}, true);
}

function liveCollapsedIds(state: AdapterLiveState): SessionId[] {
  return Object.values(state)
    .filter(record => record.phase !== 'ended')
    .map(record => record.sessionId);
}

/** Open adapter pages plus folded-away ones no page has taken back yet. */
export function countAdapterSessions(
  pages: AdapterPages,
  collapsed: readonly SessionId[],
): number {
  const shown = new Set(Object.values(pages));
  return (
    Object.keys(pages).length + collapsed.filter(id => !shown.has(id)).length
  );
}

/**
 * Host sessions, open SSH sessions and adapter sessions. The session tab's
 * badge and the screen-awake / background keep-alive both follow it.
 */
export function useLiveSessionCount(): number {
  const remote = useSessions(state => selectLive(state).length);
  const ssh = useSshSessions(state => selectLiveSsh(state).length);
  const pages = adapterPages();
  const collapsed = useAdapterLive(liveCollapsedIds);
  return remote + ssh + countAdapterSessions(pages, collapsed);
}
