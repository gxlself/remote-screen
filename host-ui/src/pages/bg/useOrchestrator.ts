import { useEffect, useRef } from 'react';

import { SETUP_DONE_KEY, readFlag } from '../../app/storage';
import { zh } from '../../i18n';
import { PAIR_STATUS, type PairingResultEvent } from '../../ipc/events';
import { useIpcConnected, useIpcEvent, useIpcQuery } from '../../ipc/react';
import type { SessionSummary } from '../../ipc/types';
import { usePlatform } from '../../platform/react';

import { diffSessions, trayStateOf } from './logic';

const QUERY_REFETCH_ON = ['event.sessionsChanged', 'event.handoff'] as const;
const UPDATE_REFETCH_ON = ['event.updateState'] as const;

function pairingTitle(payload: PairingResultEvent): string {
  switch (payload.method) {
    case 'file':
      return zh.bg.pairedFileTitle;
    case 'password':
      return payload.account === undefined || payload.account.length === 0
        ? zh.bg.pairedPasswordNoAccountTitle
        : zh.bg.pairedPasswordTitle(payload.account);
    case 'approve':
      return zh.bg.pairedApproveTitle(payload.device_name ?? '');
    case 'code':
    case undefined:
      return zh.bg.pairedTitle;
  }
}

export function useOrchestrator(): void {
  const platform = usePlatform();
  const connected = useIpcConnected();
  const sessionsQuery = useIpcQuery('sessions.list', { refetchOn: [...QUERY_REFETCH_ON] });
  const updateQuery = useIpcQuery('update.status', { refetchOn: [...UPDATE_REFETCH_ON] });

  const previousSessionsRef = useRef<SessionSummary[] | null>(null);
  const capsuleVisibleRef = useRef(false);
  const trayStateRef = useRef<string | null>(null);
  const setupCheckedRef = useRef(false);

  const sessions = sessionsQuery.data?.sessions;
  const sessionCount = sessions?.length ?? 0;
  const updateVersion = updateQuery.data?.version ?? null;

  useEffect(() => {
    if (!connected) {
      if (capsuleVisibleRef.current !== false) {
        capsuleVisibleRef.current = false;
        void platform.hideWindow('capsule');
        void platform.hideWindow('border');
      }
      return;
    }
    if (sessions === undefined) {
      return;
    }

    const hasSessions = sessionCount > 0;
    if (capsuleVisibleRef.current === hasSessions) {
      return;
    }
    capsuleVisibleRef.current = hasSessions;

    if (hasSessions) {
      void platform.showWindow('capsule');
      void platform.showWindow('border');
    } else {
      void platform.hideWindow('capsule');
      void platform.hideWindow('border');
    }
  }, [connected, platform, sessionCount, sessions]);

  useEffect(() => {
    if (connected && (sessionsQuery.loading || updateQuery.loading)) {
      return;
    }

    const state = trayStateOf({
      connected,
      sessions: sessionCount,
      updateVersion,
    });
    if (trayStateRef.current === state) {
      return;
    }
    trayStateRef.current = state;
    void platform.setTrayState(state);
  }, [
    connected,
    platform,
    sessionCount,
    sessionsQuery.loading,
    updateQuery.loading,
    updateVersion,
  ]);

  useEffect(() => {
    if (sessions === undefined) {
      return;
    }

    const previous = previousSessionsRef.current;
    if (previous === null) {
      previousSessionsRef.current = sessions;
      if (!setupCheckedRef.current) {
        setupCheckedRef.current = true;
        if (readFlag(SETUP_DONE_KEY) !== '1') {
          void platform.showWindow('setup', 'setup');
        }
      }
      return;
    }

    if (previous !== sessions) {
      for (const notice of diffSessions(previous, sessions)) {
        void platform.notify(notice.title, notice.body);
      }
      previousSessionsRef.current = sessions;
    }
  }, [platform, sessions]);

  useIpcEvent('event.pairingResult', (payload) => {
    if (payload.status === PAIR_STATUS.OK) {
      void platform.notify(pairingTitle(payload), zh.bg.pairedBody);
    }
  });
}
