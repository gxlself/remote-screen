import { formatDayHeader, formatHm, formatRelativePast } from './time';

const NOW = 1792465134000000;
const MINUTE = 60 * 1000 * 1000;
const HOUR = 60 * MINUTE;

describe('time formatting', () => {
  it.each([
    [NOW - 30 * 1000 * 1000, '刚刚'],
    [NOW - 5 * MINUTE, '5 分钟前'],
    [NOW - 2 * HOUR, '2 小时前'],
    [NOW - 24 * HOUR, '昨天'],
    [NOW - 3 * 24 * HOUR, '10月17日'],
  ])('formats relative time %s', (value, expected) => {
    expect(formatRelativePast(value, NOW, 480)).toBe(expected);
  });

  it('formats local HH:mm', () => {
    expect(formatHm(NOW, 480)).toBe('10:58');
  });

  it.each([
    [NOW, '今天 · 10月20日 周二'],
    [NOW - 24 * HOUR, '昨天 · 10月19日 周一'],
    [NOW - 2 * 24 * HOUR, '10月18日 周日'],
  ])('formats a day header for %s', (value, expected) => {
    expect(formatDayHeader(value, NOW, 480)).toBe(expected);
  });
});
