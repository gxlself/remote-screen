import { useEffect, useMemo, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { Directory, SshServer } from '../../native/rscore/types';
import { useAdapterLiveKeeper } from '../adapters/useAdapterLiveKeeper';
import { useCoreEvent } from '../events/EventsProvider';
import { qk } from '../data/queryClient';
import { useComputerEntries } from '../native/useComputerEntries';
import type { SessionRecord } from '../session/store';
import { selectLive, useSessions } from '../session/store';
import { useKv } from '../storage/StorageContext';
import { appendHistory, recoverUnclean, saveLiveSnapshot } from './history';
import type { HistoryEntry, LiveSnapshotEntry } from './history';
import { selectLiveSsh, sshStore, useSshSessions } from './sshStore';
import type { SshRecord } from './sshStore';

function durationS(startedAtUs: number, endedAtUs: number): number {
  return Math.max(0, Math.floor((endedAtUs - startedAtUs) / 1e6));
}

function remoteHistoryEntry(
  record: SessionRecord,
  name: string,
  endedAtUs: number,
): HistoryEntry {
  const entry: HistoryEntry = {
    id: `remote:${record.id}`,
    kind: 'remote',
    targetId: record.hostId,
    name,
    startedAtUs: record.startedAt,
    endedAtUs,
    durationS: durationS(record.startedAt, endedAtUs),
  };
  if (record.path !== null) {
    entry.path = record.path;
  }
  if (record.derpRegion !== null) {
    entry.derpRegion = record.derpRegion;
  }
  if (record.lastError?.reason !== undefined) {
    entry.endReason = record.lastError.reason;
  }
  if (record.lastError?.code !== undefined) {
    entry.code = record.lastError.code;
  }
  return entry;
}

function sshHistoryEntry(
  record: SshRecord,
  name: string,
  endedAtUs: number,
): HistoryEntry | null {
  if (record.openedAtUs === null) {
    return null;
  }
  return {
    id: `ssh:${record.sshId}`,
    kind: 'ssh',
    targetId: record.serverId,
    name,
    startedAtUs: record.openedAtUs,
    endedAtUs,
    durationS: durationS(record.openedAtUs, endedAtUs),
    endReason: record.state,
  };
}

function remoteSnapshotEntry(
  record: SessionRecord,
  name: string,
): LiveSnapshotEntry {
  const entry: LiveSnapshotEntry = {
    id: `remote:${record.id}`,
    kind: 'remote',
    targetId: record.hostId,
    name,
    startedAtUs: record.startedAt,
  };
  if (record.path !== null) {
    entry.path = record.path;
  }
  if (record.derpRegion !== null) {
    entry.derpRegion = record.derpRegion;
  }
  return entry;
}

function sshSnapshotEntry(record: SshRecord, name: string): LiveSnapshotEntry {
  return {
    id: `ssh:${record.sshId}`,
    kind: 'ssh',
    targetId: record.serverId,
    name,
    startedAtUs: record.openedAtUs ?? record.updatedAtUs,
  };
}

/**
 * The name the computer list shows (H05): local entries first, so history
 * reads right without the Hub, then the cached directory, then the session's.
 */
function hostName(
  names: ReadonlyMap<string, string>,
  queryClient: ReturnType<typeof useQueryClient>,
  hostId: string,
  fallback: string,
): string {
  const listed = names.get(hostId);
  if (listed !== undefined) {
    return listed;
  }
  const directory = queryClient.getQueryData<Directory>(qk.directory);
  return (
    directory?.computers.find(computer => computer.hostId === hostId)?.name ??
    fallback
  );
}

function serverName(
  queryClient: ReturnType<typeof useQueryClient>,
  serverId: string,
): string {
  const servers = queryClient.getQueryData<SshServer[]>(qk.sshServers);
  return (
    servers?.find(server => server.serverId === serverId)?.name ?? serverId
  );
}

export default function HistoryRecorder() {
  const kv = useKv();
  const queryClient = useQueryClient();
  const sessions = useSessions(state => state);
  const sshState = useSshSessions(state => state);
  const streamingSeen = useRef(new Set<string>());
  const savedSnapshotKey = useRef<string | null>(null);
  const entries = useComputerEntries();
  const names = useMemo(() => {
    const map = new Map<string, string>();
    for (const entry of entries) {
      if (entry.kind === 'host') {
        map.set(entry.computer.hostId, entry.computer.name);
      }
    }
    return map;
  }, [entries]);

  useEffect(() => {
    recoverUnclean(kv, Date.now() * 1000);
  }, [kv]);
  // Folded-away Screen Sharing / Remote Desktop sessions (V33).
  useAdapterLiveKeeper();

  useCoreEvent('session.state', event => {
    if (event.phase === 'streaming') {
      streamingSeen.current.add(event.sessionId);
    }
  });

  useCoreEvent('ssh.state', event => {
    const nowUs = Date.now() * 1000;
    sshStore.apply(event, nowUs);
    const record = sshStore.getState()[event.sshId];

    if (event.state === 'open') {
      return;
    }
    if (event.state !== 'closed' && event.state !== 'failed') {
      return;
    }

    if (record === undefined) {
      return;
    }
    if (record.openedAtUs !== null) {
      const entry = sshHistoryEntry(
        record,
        serverName(queryClient, record.serverId),
        nowUs,
      );
      if (entry !== null) {
        appendHistory(kv, entry);
      }
    }
    sshStore.remove(event.sshId);
  });

  useEffect(() => {
    const endedAtUs = Date.now() * 1000;
    for (const record of Object.values(sessions)) {
      if (record.phase === 'streaming') {
        streamingSeen.current.add(record.id);
        continue;
      }
      if (record.phase !== 'ended' || !streamingSeen.current.has(record.id)) {
        continue;
      }

      streamingSeen.current.delete(record.id);
      appendHistory(
        kv,
        remoteHistoryEntry(
          record,
          hostName(names, queryClient, record.hostId, record.hostName),
          endedAtUs,
        ),
      );
    }
  }, [kv, names, queryClient, sessions]);

  useEffect(() => {
    const snapshot: LiveSnapshotEntry[] = [];
    for (const record of selectLive(sessions)) {
      if (
        record.phase !== 'streaming' &&
        !streamingSeen.current.has(record.id)
      ) {
        continue;
      }
      snapshot.push(
        remoteSnapshotEntry(
          record,
          hostName(names, queryClient, record.hostId, record.hostName),
        ),
      );
    }
    for (const record of selectLiveSsh(sshState)) {
      if (record.state === 'open') {
        snapshot.push(
          sshSnapshotEntry(record, serverName(queryClient, record.serverId)),
        );
      }
    }
    const snapshotKey = JSON.stringify(snapshot);
    if (savedSnapshotKey.current === snapshotKey) {
      return;
    }
    savedSnapshotKey.current = snapshotKey;
    saveLiveSnapshot(kv, snapshot);
  }, [kv, names, queryClient, sessions, sshState]);

  return null;
}
