import { RsCoreError } from '../../native/rscore/RsCoreApi';
import {
  capLabel,
  formatBytes,
  formatDuration,
  formatStartedAt,
  listErrorText,
  retentionLabel,
  stateBadge,
  storageSummary,
} from './library';

describe('recording library formatting', () => {
  it('formats bytes in binary units', () => {
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(5 * 2 ** 30)).toBe('5.0 GB');
  });

  it('formats durations with hours when needed', () => {
    expect(formatDuration(5_000)).toBe('0:05');
    expect(formatDuration(754_000)).toBe('12:34');
    expect(formatDuration(3_723_000)).toBe('1:02:03');
  });

  it('uses today and yesterday for local calendar dates', () => {
    const now = new Date(2025, 2, 2, 15, 10).getTime();
    expect(
      formatStartedAt(new Date(2025, 2, 2, 14, 5).getTime() * 1000, now),
    ).toBe('今天 14:05');
    expect(
      formatStartedAt(new Date(2025, 2, 1, 9, 30).getTime() * 1000, now),
    ).toBe('昨天 09:30');
  });

  it('maps recording state to badges', () => {
    expect(stateBadge('complete')).toBeNull();
    expect(stateBadge('recovered')).toEqual({
      label: '已恢复',
      tone: 'neutral',
    });
    expect(stateBadge('damaged')).toEqual({ label: '已损坏', tone: 'bad' });
    expect(stateBadge('recording')).toEqual({
      label: '录制中',
      tone: 'neutral',
    });
  });

  it('formats storage and settings values', () => {
    expect(
      storageSummary({
        usedBytes: 1.2 * 2 ** 30,
        capBytes: 4 * 2 ** 30,
        freeBytes: 20.5 * 2 ** 30,
      }),
    ).toBe('已用 1.2 GB / 4 GB · 手机剩余 20.5 GB');
    expect(storageSummary({ usedBytes: 1536, capBytes: 4 * 2 ** 30 })).toBe(
      '已用 1.5 KB / 4 GB',
    );
    expect(retentionLabel(null)).toBe('永久保留');
    expect(capLabel(8192)).toBe('8 GB');
  });

  it('recognizes key-unavailable errors', () => {
    expect(
      listErrorText(new RsCoreError('internal', 'recording.keyUnavailable')),
    ).toBe('暂时无法解锁录制文件');
  });
});
