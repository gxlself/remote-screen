import { useEffect, useMemo, useState } from 'react';
import {
  selectCollapsed,
  useAdapterLive,
  type AdapterRouteParams,
  type CollapsedAdapter,
} from '../../features/adapters/liveStore';
import { useDirectory, useSshServers } from '../../features/data/queries';
import { formatClock } from '../../features/format/duration';
import { formatFps } from '../../features/format/latency';
import { selectLive, useSessions } from '../../features/session/store';
import type { SessionRecord } from '../../features/session/store';
import { useHistory } from '../../features/sessions/history';
import type { HistoryEntry } from '../../features/sessions/history';
import { historyLine } from '../../features/sessions/historyText';
import {
  adapterStatus,
  remoteStatus,
} from '../../features/sessions/remoteStatus';
import type { HistoryLine } from '../../features/sessions/historyText';
import {
  selectLiveSsh,
  useSshSessions,
} from '../../features/sessions/sshStore';
import type { SshRecord } from '../../features/sessions/sshStore';
import { useSetting } from '../../features/settings/store';
import { sessions as sessionsText } from '../../i18n/zh/sessions';
import type { NetPath, SessionDisplay } from '../../native/rscore/types';

export type RemoteSessionItem = {
  id: string;
  hostId: string;
  name: string;
  elapsed: string;
  statusLabel: string;
  statusRelay: boolean;
  path: NetPath | null;
  latencyMs: number | null;
  fpsLabel: string | null;
  displayLabel: string | null;
  /** 屏幕共享 / 远程桌面, shown where a Host session shows its path. */
  kindLabel?: string;
};

/** A folded-away Screen Sharing / Remote Desktop session (V33). */
export type AdapterSessionItem = RemoteSessionItem & {
  params: AdapterRouteParams;
  /** The 最近 identity, so a history row returns to it. */
  targetId: string;
};

export type SshSessionItem = {
  sshId: string;
  serverId: string;
  name: string;
  elapsed: string;
  connecting: boolean;
  subtitle: string;
};

export type HistoryItem = HistoryLine & {
  id: string;
  kind: HistoryEntry['kind'];
  targetId: string;
};

export type SessionsModel = {
  remote: RemoteSessionItem[];
  adapters: AdapterSessionItem[];
  ssh: SshSessionItem[];
  history: HistoryItem[];
  liveCount: number;
};

function secondsSince(startedAtUs: number | null, nowUs: number): number {
  if (startedAtUs === null) {
    return 0;
  }
  return Math.max(0, Math.floor((nowUs - startedAtUs) / 1_000_000));
}

function displayLabel(
  displays: SessionDisplay[],
  currentId: number | null,
): string | null {
  if (displays.length <= 1) {
    return null;
  }
  const current =
    displays.find(display => display.id === currentId) ??
    displays.find(display => display.primary) ??
    displays[0];
  return sessionsText.remote.display(
    Math.max(1, displays.indexOf(current) + 1),
    displays.length,
  );
}

function remoteItem(
  record: SessionRecord,
  name: string,
  nowUs: number,
): RemoteSessionItem {
  const stats = record.stats;
  const status = remoteStatus(record.phase, record.role);
  return {
    id: record.id,
    hostId: record.hostId,
    name,
    elapsed: formatClock(secondsSince(record.startedAt, nowUs)),
    statusLabel: status.label,
    statusRelay: status.relay,
    path: stats?.path ?? null,
    latencyMs: stats?.rttMs ?? null,
    fpsLabel: stats === null ? null : formatFps(stats.fps),
    displayLabel: displayLabel(record.displays, record.displayId),
  };
}

function adapterItem(
  record: CollapsedAdapter,
  nowUs: number,
): AdapterSessionItem {
  const status = adapterStatus(record.phase);
  return {
    id: record.sessionId,
    hostId: record.params.target.hostId ?? '',
    name: record.name,
    elapsed: formatClock(secondsSince(record.startedAtUs, nowUs)),
    statusLabel: status.label,
    statusRelay: status.relay,
    path: null,
    latencyMs: null,
    fpsLabel: null,
    displayLabel: null,
    kindLabel: sessionsText.history.adapterKind[record.params.target.protocol],
    params: record.params,
    targetId: record.history.targetId,
  };
}

function sshItem(
  record: SshRecord,
  name: string,
  tmux: boolean,
  nowUs: number,
): SshSessionItem {
  const connecting =
    record.state === 'connecting' || record.state === 'authenticating';
  return {
    sshId: record.sshId,
    serverId: record.serverId,
    name,
    elapsed: formatClock(secondsSince(record.openedAtUs, nowUs)),
    connecting,
    subtitle: connecting
      ? sessionsText.ssh.connecting
      : tmux
      ? sessionsText.ssh.tmux
      : sessionsText.ssh.plain,
  };
}

export function useSessionsModel(): SessionsModel {
  const { directory } = useDirectory();
  const serversQuery = useSshServers();
  const liveSessions = useSessions(selectLive);
  const liveSsh = useSshSessions(selectLiveSsh);
  const collapsed = useAdapterLive(selectCollapsed);
  const history = useHistory();
  const [tmux] = useSetting('terminal.tmux');
  const [nowUs, setNowUs] = useState(() => Date.now() * 1000);

  useEffect(() => {
    const timer = setInterval(() => setNowUs(Date.now() * 1000), 1000);
    return () => clearInterval(timer);
  }, []);

  const hostNames = useMemo(
    () =>
      new Map(
        (directory?.computers ?? []).map(computer => [
          computer.hostId,
          computer.name,
        ]),
      ),
    [directory],
  );
  const serverNames = useMemo(
    () =>
      new Map(
        (serversQuery.data ?? []).map(server => [server.serverId, server.name]),
      ),
    [serversQuery.data],
  );

  const remote = useMemo(
    () =>
      liveSessions.map(record =>
        remoteItem(
          record,
          hostNames.get(record.hostId) ?? record.hostId,
          nowUs,
        ),
      ),
    [hostNames, liveSessions, nowUs],
  );
  const adapters = useMemo(
    () => collapsed.map(record => adapterItem(record, nowUs)),
    [collapsed, nowUs],
  );
  const ssh = useMemo(
    () =>
      liveSsh.map(record =>
        sshItem(
          record,
          serverNames.get(record.serverId) ?? record.serverId,
          tmux,
          nowUs,
        ),
      ),
    [liveSsh, nowUs, serverNames, tmux],
  );
  const historyItems = useMemo(
    () =>
      history.map(entry => ({
        ...historyLine(entry, nowUs),
        id: entry.id,
        kind: entry.kind,
        targetId: entry.targetId,
      })),
    [history, nowUs],
  );

  return {
    remote,
    adapters,
    ssh,
    history: historyItems,
    liveCount: remote.length + adapters.length + ssh.length,
  };
}
