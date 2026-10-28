import type { PairFileSummary } from '../../native/rscore/types';

const CACHE_TTL_MS = 10 * 60 * 1000;
const MAX_ENTRIES = 8;

type CacheEntry = {
  summary: PairFileSummary;
  storedAtMs: number;
};

const entries = new Map<string, CacheEntry>();

function copySummary(summary: PairFileSummary): PairFileSummary {
  return {
    fileId: summary.fileId,
    hostId: summary.hostId,
    hostName: summary.hostName,
    fp: summary.fp,
    addrs: [...summary.addrs],
    expiresAt: summary.expiresAt,
    expired: summary.expired,
  };
}

function pruneExpired(nowMs: number): void {
  for (const [fileId, entry] of entries) {
    if (nowMs - entry.storedAtMs >= CACHE_TTL_MS) {
      entries.delete(fileId);
    }
  }
}

export function rememberPairFile(
  summary: PairFileSummary,
  nowMs = Date.now(),
): void {
  pruneExpired(nowMs);
  entries.delete(summary.fileId);
  entries.set(summary.fileId, {
    summary: copySummary(summary),
    storedAtMs: nowMs,
  });
  while (entries.size > MAX_ENTRIES) {
    const oldest = entries.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    entries.delete(oldest);
  }
}

export function recallPairFile(
  fileId: string,
  nowMs = Date.now(),
): PairFileSummary | undefined {
  const entry = entries.get(fileId);
  if (entry === undefined) {
    return undefined;
  }
  if (nowMs - entry.storedAtMs >= CACHE_TTL_MS) {
    entries.delete(fileId);
    return undefined;
  }
  return copySummary(entry.summary);
}

export function clearPairFiles(): void {
  entries.clear();
}
