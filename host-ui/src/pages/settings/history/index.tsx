import { useCallback, useMemo, useState } from 'react';

import { zh } from '../../../i18n';
import { useIpc, useIpcEvent, useIpcQuery } from '../../../ipc';
import { formatDateTime, formatDurationS, pathLabel } from '../../../lib';
import { Banner, Button } from '../../../components';

import styles from './history.module.css';

const PAGE_SIZE = 50;

interface AuditEntry {
  ts: number;
  user: string | null;
  device_id: string | null;
  event: string | null;
  path: string | null;
  duration_s: number | null;
}

type DeviceNames = ReadonlyMap<string, string>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseEntry(value: unknown): AuditEntry | null {
  if (!isRecord(value) || typeof value.ts !== 'number' || !Number.isFinite(value.ts)) {
    return null;
  }
  return {
    ts: value.ts,
    user: typeof value.user === 'string' ? value.user : null,
    device_id: typeof value.device_id === 'string' ? value.device_id : null,
    event: typeof value.event === 'string' ? value.event : null,
    path: typeof value.path === 'string' ? value.path : null,
    duration_s: typeof value.duration_s === 'number' ? value.duration_s : null,
  };
}

function parseEntries(values: readonly unknown[]): AuditEntry[] {
  return values.map(parseEntry).filter((entry): entry is AuditEntry => entry !== null);
}

function dedupeByTs(entries: readonly AuditEntry[]): AuditEntry[] {
  const seen = new Set<number>();
  return entries.filter((entry) => {
    if (seen.has(entry.ts)) {
      return false;
    }
    seen.add(entry.ts);
    return true;
  });
}

function eventLabel(event: string | null): { label: string; bad: boolean } {
  if (event === null) {
    return { label: '—', bad: false };
  }
  if (event.startsWith('security.')) {
    return { label: zh.history.events.security, bad: true };
  }
  if (event.startsWith('upgrade.')) {
    return { label: zh.history.events.upgrade, bad: false };
  }
  switch (event) {
    case 'session.start':
      return { label: zh.history.events['session.start'], bad: false };
    case 'session.end':
      return { label: zh.history.events['session.end'], bad: false };
    case 'auth.decision':
      return { label: zh.history.events['auth.decision'], bad: false };
    case 'path.switch':
      return { label: zh.history.events['path.switch'], bad: false };
    case 'trust.apply':
      return { label: zh.history.events['trust.apply'], bad: false };
    default:
      return { label: event, bad: false };
  }
}

function deviceName(entry: AuditEntry, devices: DeviceNames): string | null {
  if (entry.device_id === null) {
    return null;
  }
  return devices.get(entry.device_id) ?? entry.device_id;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

interface HistoryTableProps {
  firstPage: readonly AuditEntry[];
  firstPageCount: number;
  deviceNames: DeviceNames;
}

function HistoryTable({ firstPage, firstPageCount, deviceNames }: HistoryTableProps) {
  const ipc = useIpc();
  const [olderEntries, setOlderEntries] = useState<AuditEntry[]>([]);
  const [loadingMore, setLoadingMore] = useState(false);
  const [loadMoreError, setLoadMoreError] = useState<string | null>(null);
  const [nextPageCount, setNextPageCount] = useState<number | null>(null);
  const entries = useMemo(
    () => dedupeByTs([...firstPage, ...olderEntries]),
    [firstPage, olderEntries],
  );
  const lastTs = entries.length > 0 ? entries[entries.length - 1]?.ts : undefined;
  const moreAvailable = (nextPageCount ?? firstPageCount) === PAGE_SIZE;

  const loadMore = useCallback(async (): Promise<void> => {
    if (lastTs === undefined || loadingMore) {
      return;
    }
    setLoadingMore(true);
    setLoadMoreError(null);
    try {
      const next = await ipc.call('audit.list', {
        limit: PAGE_SIZE,
        before_us: lastTs,
      });
      setOlderEntries((previous) => dedupeByTs([...previous, ...parseEntries(next.entries)]));
      setNextPageCount(next.entries.length);
    } catch (error) {
      setLoadMoreError(errorMessage(error));
    } finally {
      setLoadingMore(false);
    }
  }, [ipc, lastTs, loadingMore]);

  return (
    <>
      <div className={styles.tableWrap}>
        <table className={styles.table}>
          <thead>
            <tr>
              <th>{zh.history.columns.time}</th>
              <th>{zh.history.columns.device}</th>
              <th>{zh.history.columns.event}</th>
              <th>{zh.history.columns.path}</th>
              <th>{zh.history.columns.duration}</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry, index) => {
              const name = deviceName(entry, deviceNames);
              const event = eventLabel(entry.event);
              return (
                <tr key={`${entry.ts}-${index}`}>
                  <td className={styles.mono}>{formatDateTime(entry.ts)}</td>
                  <td>
                    {name !== null ? (
                      <span className={styles.device}>
                        <span className={name === entry.device_id ? styles.mono : undefined}>
                          {name}
                        </span>
                        {entry.user !== null ? (
                          <span className={styles.user}>{entry.user}</span>
                        ) : null}
                      </span>
                    ) : (
                      zh.history.deviceFallback
                    )}
                  </td>
                  <td className={event.bad ? styles.badText : undefined}>{event.label}</td>
                  <td className={styles.secondary}>{pathLabel(entry.path)}</td>
                  <td className={styles.secondary}>{formatDurationS(entry.duration_s)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {loadMoreError !== null ? (
        <div className={styles.loadMoreError}>
          <Banner tone="bad" role="alert">
            {zh.history.errorPrefix}
            {loadMoreError}
          </Banner>
        </div>
      ) : null}

      {moreAvailable ? (
        <div className={styles.more}>
          <Button
            disabled={loadingMore}
            size="sm"
            variant="secondary"
            onClick={() => void loadMore()}
          >
            {zh.history.loadMore}
          </Button>
        </div>
      ) : null}
    </>
  );
}

export default function HistorySection() {
  const audit = useIpcQuery('audit.list', {
    params: { limit: PAGE_SIZE },
    refetchOn: ['event.sessionsChanged'],
  });
  const devices = useIpcQuery('devices.list');
  const [generation, setGeneration] = useState(0);

  useIpcEvent('event.sessionsChanged', () => {
    setGeneration((previous) => previous + 1);
  });

  const rawEntries = audit.data?.entries;
  const firstPage = useMemo(
    () => parseEntries(Array.isArray(rawEntries) ? rawEntries : []),
    [rawEntries],
  );

  const deviceNames = useMemo<DeviceNames>(() => {
    const names = new Map<string, string>();
    for (const device of devices.data?.devices ?? []) {
      names.set(device.device_id, device.name);
    }
    return names;
  }, [devices.data]);
  const firstPageCount = Array.isArray(rawEntries) ? rawEntries.length : 0;

  const auditRefetch = audit.refetch;
  const devicesRefetch = devices.refetch;
  const retry = useCallback((): void => {
    void auditRefetch();
    void devicesRefetch();
  }, [auditRefetch, devicesRefetch]);

  return (
    <section className={styles.section} data-section="history">
      <h2 className={styles.title}>{zh.history.title}</h2>
      <p className={styles.description}>{zh.history.description}</p>

      {audit.error !== undefined ? (
        <div className={styles.error}>
          <Banner tone="bad" role="alert">
            {zh.history.errorPrefix}
            {errorMessage(audit.error)}
          </Banner>
          <div className={styles.errorActions}>
            <Button size="sm" variant="secondary" onClick={retry}>
              {zh.common.retry}
            </Button>
          </div>
        </div>
      ) : audit.loading ? (
        <p className={styles.empty}>{zh.common.loading}</p>
      ) : firstPageCount === 0 ? (
        <p className={styles.empty}>{zh.history.empty}</p>
      ) : (
        <HistoryTable
          deviceNames={deviceNames}
          firstPage={firstPage}
          firstPageCount={firstPageCount}
          key={generation}
        />
      )}
    </section>
  );
}
