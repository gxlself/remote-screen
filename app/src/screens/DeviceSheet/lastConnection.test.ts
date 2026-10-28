import type { HistoryEntry } from '../../features/sessions/history';
import { formatLastConnection } from './useDeviceSheet';

const TZ_OFFSET_MIN = 8 * 60;
const MINUTE_US = 60 * 1_000_000;
const HOUR_US = 60 * MINUTE_US;

function localUs(
  year: number,
  monthIndex: number,
  day: number,
  hour: number,
  minute: number,
): number {
  return (
    Date.UTC(year, monthIndex, day, hour - TZ_OFFSET_MIN / 60, minute) * 1_000
  );
}

const NOW_US = localUs(2026, 9, 20, 10, 58);

/** A remote session this phone ran, as the local history keeps it. */
function session(
  endedAtUs: number,
  overrides: Partial<HistoryEntry> = {},
): HistoryEntry {
  return {
    id: 'session-1',
    kind: 'remote',
    targetId: 'h7v2m9q3k8x4tb1a',
    name: 'Studio Mac mini',
    startedAtUs: endedAtUs - 42 * MINUTE_US,
    endedAtUs,
    durationS: 42 * 60,
    path: 'tailnetDirect',
    ...overrides,
  };
}

it('shows the end time and duration of a session that ended today', () => {
  expect(formatLastConnection(session(NOW_US), NOW_US, TZ_OFFSET_MIN)).toBe(
    '今天 10:58 · 42 分钟',
  );
  expect(
    formatLastConnection(session(NOW_US - 3 * HOUR_US), NOW_US, TZ_OFFSET_MIN),
  ).toBe('今天 07:58 · 42 分钟');
});

it('dates a session by when it ended, across midnight', () => {
  const now = localUs(2026, 9, 20, 0, 20);
  const endedAt = localUs(2026, 9, 19, 23, 50);

  expect(formatLastConnection(session(endedAt), now, TZ_OFFSET_MIN)).toBe(
    '昨天 23:50 · 42 分钟',
  );
});

it('uses the month and day for older sessions', () => {
  const endedAt = localUs(2026, 9, 16, 9, 5);

  expect(
    formatLastConnection(
      session(endedAt, { durationS: 90 }),
      NOW_US,
      TZ_OFFSET_MIN,
    ),
  ).toBe('10月16日 09:05 · 1 分钟');
});

it('uses the recovery time for a session the app did not end cleanly', () => {
  // recoverUnclean ends a session at the next launch and keeps its duration.
  const recoveredAt = NOW_US - 2 * HOUR_US;

  expect(
    formatLastConnection(
      session(recoveredAt, { unclean: true, durationS: 5 * 60 }),
      NOW_US,
      TZ_OFFSET_MIN,
    ),
  ).toBe('今天 08:58 · 5 分钟');
});
