import { formatClock, formatDurationWords } from './duration';

describe('duration formatting', () => {
  it.each([
    [2538, '42:18'],
    [3764, '1:02:44'],
    [0, '00:00'],
  ])('formats %s seconds as clock %s', (value, expected) => {
    expect(formatClock(value)).toBe(expected);
  });

  it.each([
    [2520, '42 分钟'],
    [4320, '1 小时 12 分'],
    [20, '1 分钟'],
    [3660, '1 小时 1 分'],
  ])('formats %s seconds as words %s', (value, expected) => {
    expect(formatDurationWords(value)).toBe(expected);
  });
});
