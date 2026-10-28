import { useCallback, useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { PathKind } from '../../components/PathMark';
import type { StatusChipTone } from '../../components/StatusChip';
import { useHostPath } from '../../features/directory/pathStore';
import { sortDisplays } from '../../features/directory/displayLabel';
import { isHostHistory } from '../../features/directory/wideRows';
import { formatMs } from '../../features/format/latency';
import { formatDurationWords } from '../../features/format/duration';
import { formatHm } from '../../features/format/time';
import { useDirectory } from '../../features/data/queries';
import { qk } from '../../features/data/queryClient';
import { selectLive, useSessions } from '../../features/session/store';
import { useHistory } from '../../features/sessions/history';
import type { HistoryEntry } from '../../features/sessions/history';
import type { ConnectVia } from '../../features/native/types';
import { useHostPrefs, useSetting } from '../../features/settings/store';
import { toggleSensitive } from '../../features/settings/schema';
import { common } from '../../i18n/zh/common';
import { deviceSheet } from '../../i18n/zh/deviceSheet';
import { useCore } from '../../native/rscore/CoreContext';
import type { Address, HostDisplay } from '../../native/rscore/types';

export type DeviceSheetHost = {
  hostId: string;
  name: string;
  node: string;
  os: 'macos' | 'windows';
  osVersion: string;
  hostVersion: string;
  online: boolean;
  addresses: Address[];
  displays: HostDisplay[];
};

export type DeviceSheetPathChip = {
  label: string;
  tone: StatusChipTone;
  mark: PathKind | undefined;
};

function localDate(us: number, tzOffsetMin?: number): Date {
  const offsetMin =
    tzOffsetMin === undefined ? -new Date().getTimezoneOffset() : tzOffsetMin;
  return new Date(Math.floor(us / 1000) + offsetMin * 60_000);
}

function localDayKey(date: Date): string {
  return `${date.getUTCFullYear()}-${date.getUTCMonth()}-${date.getUTCDate()}`;
}

function connectionDayWord(
  ts: number,
  nowUs: number,
  tzOffsetMin?: number,
): string {
  const date = localDate(ts, tzOffsetMin);
  const now = localDate(nowUs, tzOffsetMin);
  if (localDayKey(date) === localDayKey(now)) {
    return common.time.today;
  }

  const yesterday = new Date(
    now.getUTCFullYear(),
    now.getUTCMonth(),
    now.getUTCDate() - 1,
  );
  if (localDayKey(date) === localDayKey(yesterday)) {
    return common.time.yesterday;
  }

  return common.time.monthDay(date.getUTCMonth() + 1, date.getUTCDate());
}

/** When this phone's last session with the Host ended, and how long it ran. */
export function formatLastConnection(
  entry: HistoryEntry,
  nowUs: number,
  tzOffsetMin?: number,
): string {
  const day = connectionDayWord(entry.endedAtUs, nowUs, tzOffsetMin);
  const time = formatHm(entry.endedAtUs, tzOffsetMin);
  return `${day} ${time} · ${formatDurationWords(entry.durationS)}`;
}

export function useDeviceSheet(hostId: string) {
  const core = useCore();
  const queryClient = useQueryClient();
  const directoryQuery = useDirectory();
  const hostPath = useHostPath(hostId);
  const liveSessions = useSessions(selectLive);
  const history = useHistory();
  const [sensitiveHosts, setSensitiveHosts] = useSetting('lock.sensitiveHosts');
  const [prefs, setHostPrefs] = useHostPrefs(hostId);
  const [selectedDisplayId, setSelectedDisplayId] = useState<number | null>(
    null,
  );

  const computer = useMemo(
    () =>
      directoryQuery.directory?.computers.find(
        candidate => candidate.hostId === hostId,
      ),
    [directoryQuery.directory, hostId],
  );
  const displays = useMemo(
    () => sortDisplays(computer?.displays ?? []),
    [computer?.displays],
  );
  const selectedDisplay = useMemo(
    () =>
      displays.find(display => display.id === selectedDisplayId) ??
      displays.find(display => display.primary) ??
      displays[0],
    [displays, selectedDisplayId],
  );
  const liveSession = liveSessions.find(session => session.hostId === hostId);
  // History is newest first; Screen Sharing / Remote Desktop count too.
  const lastConnection = history.find(entry => isHostHistory(entry, hostId));

  const host = useMemo<DeviceSheetHost | undefined>(() => {
    if (computer === undefined) {
      return undefined;
    }
    return {
      hostId: computer.hostId,
      name: computer.name,
      node: computer.node,
      os: computer.os,
      osVersion: computer.osVersion ?? '',
      hostVersion: computer.hostVersion,
      online: computer.online,
      addresses: computer.addresses,
      displays,
    };
  }, [computer, displays]);

  // Addresses are in 连接地址, not under the name.
  const subtitle = host === undefined ? '' : host.node;

  const pathChip = useMemo<DeviceSheetPathChip>(() => {
    if (hostPath !== undefined) {
      return {
        label: `${common.path[hostPath.path]} ${formatMs(hostPath.latencyMs)}`,
        tone: 'neutral',
        mark: hostPath.path,
      };
    }
    if (host?.online === true) {
      return {
        label: common.status.online,
        tone: 'good',
        mark: undefined,
      };
    }
    return {
      label: common.status.offline,
      tone: 'offline',
      mark: 'offline',
    };
  }, [host?.online, hostPath]);

  const addressSummary = useMemo(() => {
    const pathText =
      hostPath === undefined
        ? host?.online === true
          ? common.status.online
          : common.status.offline
        : common.path[hostPath.path];
    const count = host?.addresses.length ?? 0;
    return count === 0
      ? pathText
      : `${pathText} · ${deviceSheet.rows.addressCount(count)}`;
  }, [host?.addresses, host?.online, hostPath]);

  /** Null when this phone has no session with the Host on record. */
  const lastConnectionText: string | null =
    lastConnection === undefined
      ? null
      : formatLastConnection(lastConnection, Date.now() * 1000);

  const selectDisplay = useCallback((displayId: number) => {
    setSelectedDisplayId(displayId);
  }, []);

  const sensitive = sensitiveHosts.includes(hostId);
  const setSensitive = useCallback(
    (on: boolean) => {
      setSensitiveHosts(toggleSensitive(sensitiveHosts, hostId, on));
    },
    [hostId, sensitiveHosts, setSensitiveHosts],
  );

  const setLockOnEnd = useCallback(
    (on: boolean) => {
      setHostPrefs({ lockOnEnd: on });
    },
    [setHostPrefs],
  );

  const setConnectVia = useCallback(
    (connectVia: ConnectVia) => {
      setHostPrefs({ connectVia });
    },
    [setHostPrefs],
  );

  const lockSession = useCallback((): boolean => {
    if (liveSession === undefined) {
      return false;
    }
    try {
      core.sessions.action(liveSession.id, 'lock');
      return true;
    } catch {
      return false;
    }
  }, [core, liveSession]);

  const forgetHost = useCallback(async (): Promise<boolean> => {
    try {
      await core.hosts.forget(hostId);
    } catch {
      return false;
    }
    try {
      await queryClient.invalidateQueries({ queryKey: qk.directory });
    } catch {}
    return true;
  }, [core, hostId, queryClient]);

  const isLoading = host === undefined && directoryQuery.isLoading;

  return {
    host,
    /** The directory entry; `useConnect` needs its addresses and state. */
    computer,
    isLoading,
    notFound: !isLoading && host === undefined,
    subtitle,
    displays,
    selectedDisplay,
    pathChip,
    addressSummary,
    lastConnectionText,
    liveSession,
    sensitive,
    setSensitive,
    lockOnEnd: prefs.lockOnEnd,
    setLockOnEnd,
    connectVia: prefs.connectVia,
    setConnectVia,
    selectDisplay,
    lockSession,
    forgetHost,
  };
}
