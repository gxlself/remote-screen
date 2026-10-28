import {
  formatBitrate,
  formatFps,
  formatMs,
  formatResolution,
  spokenMs,
} from './latency';

describe('latency formatting', () => {
  it.each([
    [6.4, '6 ms'],
    [6.6, '7 ms'],
    [0.3, '<1 ms'],
    [undefined, '—'],
    [null, '—'],
  ] as const)('formats %s ms as %s', (value, expected) => {
    expect(formatMs(value)).toBe(expected);
  });

  it.each([
    [6.4, '6 毫秒'],
    [0.3, '<1 毫秒'],
    [undefined, '—'],
  ] as const)('speaks %s ms as %s', (value, expected) => {
    expect(spokenMs(value)).toBe(expected);
  });

  it('formats frame rate', () => {
    expect(formatFps(60)).toBe('60 fps');
  });

  it.each([
    [18400, '18.4 Mbps'],
    [850, '850 kbps'],
  ])('formats bitrate %s as %s', (value, expected) => {
    expect(formatBitrate(value)).toBe(expected);
  });

  it('formats resolution', () => {
    expect(formatResolution(2560, 1600)).toBe('2560×1600');
  });
});
