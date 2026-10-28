const WINDOW_MS = 10 * 60 * 1000;
const MAX_REQUESTS = 3;

const deniedAt = new Map<string, number[]>();

function activeRecords(hostId: string, nowMs: number): number[] {
  const records = deniedAt.get(hostId) ?? [];
  const active = records.filter(at => nowMs - at < WINDOW_MS);
  if (active.length === 0) {
    deniedAt.delete(hostId);
  } else {
    deniedAt.set(hostId, active);
  }
  return active;
}

export function recordDenied(hostId: string, nowMs: number): void {
  const records = activeRecords(hostId, nowMs);
  records.push(nowMs);
  deniedAt.set(hostId, records);
}

export function remainingRequests(hostId: string, nowMs: number): number {
  return Math.max(0, MAX_REQUESTS - activeRecords(hostId, nowMs).length);
}

export function resetDenied(): void {
  deniedAt.clear();
}
