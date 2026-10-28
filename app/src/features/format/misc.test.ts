import { errorCodeLabel, formatBytes } from './misc';

describe('misc formatting', () => {
  it('prefixes error codes', () => {
    expect(errorCodeLabel(170)).toBe('RS-E170');
  });

  it.each([
    [1288490188, '1.2 GB'],
    [64172851, '61.2 MB'],
    [524288, '512 KB'],
    [0, '0 B'],
  ])('formats %s bytes as %s', (value, expected) => {
    expect(formatBytes(value)).toBe(expected);
  });
});
