import { describe, expect, it } from 'vitest';

import {
  formatCode,
  formatCountdown,
  formatDateTime,
  formatDurationS,
  formatElapsed,
  formatFp,
  formatFpFull,
  formatWhen,
  secondsLeft,
} from './format';

const FP_BYTES = Uint8Array.from([
  0x7f, 0x3a, 0x91, 0xc0, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb,
  0xcc, 0xdd, 0xee, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x5e, 0x2b, 0xc2, 0xd4,
]);

const FP_URL = Buffer.from(FP_BYTES).toString('base64url');

function micros(date: Date): number {
  return date.getTime() * 1000;
}

describe('formatFp', () => {
  it.each([
    [
      '000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f',
      'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
      '000G 40R4 0M30 E209',
    ],
    ['ff'.repeat(32), '__________________________________________8', 'ZZZZ ZZZZ ZZZZ ZZZZ'],
    [
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
      '47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU',
      'WERC 8GMR ZGE1 96QV',
    ],
  ])('formats shared vector %s', (_hex, url, display) => {
    expect(formatFp(url)).toBe(display);
  });

  it('formats the first 10 bytes in Crockford base32', () => {
    expect(formatFp(FP_URL)).toBe('FWX9 3G00 24H3 6H2N');
  });

  it('accepts base64url padding', () => {
    expect(formatFp(`${FP_URL}=`)).toBe('FWX9 3G00 24H3 6H2N');
  });

  it('ignores bytes after the first 10 and is sensitive to the last displayed bit', () => {
    const tailChanged = Uint8Array.from(FP_BYTES);
    tailChanged.fill(0xff, 10);
    expect(formatFp(Buffer.from(tailChanged).toString('base64url'))).toBe('FWX9 3G00 24H3 6H2N');

    const lastBitChanged = Uint8Array.from(FP_BYTES);
    const lastByte = lastBitChanged[9];
    if (lastByte === undefined) {
      throw new Error('FP_BYTES is shorter than 10 bytes');
    }
    lastBitChanged[9] = lastByte ^ 1;
    expect(formatFp(Buffer.from(lastBitChanged).toString('base64url'))).toBe('FWX9 3G00 24H3 6H2M');
  });

  it.each([
    Buffer.from(FP_BYTES.slice(0, 31)).toString('base64url'),
    '*'.repeat(43),
    null,
    undefined,
    '',
  ])('returns a dash for an invalid fingerprint %s', (value) => {
    expect(formatFp(value)).toBe('—');
  });

  it.each(Array.from({ length: 256 }, (_, byte) => byte))(
    'uses the Crockford base32 display shape for repeated byte %d',
    (byte) => {
      const bytes = new Uint8Array(32).fill(byte);
      const display = formatFp(Buffer.from(bytes).toString('base64url'));

      expect(display).toHaveLength(19);
      expect(display).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}( [0-9A-HJKMNP-TV-Z]{4}){3}$/);
    },
  );

  it('formats all 16 full-fingerprint groups', () => {
    expect(formatFpFull(FP_URL).split(' ')).toHaveLength(16);
    expect(formatFpFull(FP_URL)).toBe(
      '7F3A 91C0 0011 2233 4455 6677 8899 AABB CCDD EEFF 1020 3040 5060 7080 5E2B C2D4',
    );
  });
});

describe('formatCode', () => {
  it('groups six digits', () => {
    expect(formatCode('482913')).toBe('482 913');
  });

  it('leaves other values unchanged', () => {
    expect(formatCode('12345')).toBe('12345');
    expect(formatCode('482913\n')).toBe('482913\n');
  });
});

describe('elapsed and countdown formatting', () => {
  it.each([
    [0, '0:00'],
    [2_538_000, '42:18'],
    [303_000, '5:03'],
    [3_723_000, '1:02:03'],
    [-5, '0:00'],
  ])('formats %d ms as %s', (ms, expected) => {
    expect(formatElapsed(ms)).toBe(expected);
  });

  it('rounds countdowns up to the next second', () => {
    expect(formatCountdown(276_200)).toBe('4:37');
    expect(formatCountdown(0)).toBe('0:00');
  });

  it('rounds seconds left up and clamps at zero', () => {
    expect(secondsLeft(27_001)).toBe(28);
    expect(secondsLeft(-1)).toBe(0);
  });
});

describe('formatWhen', () => {
  const now = new Date(2026, 9, 20, 15, 0);

  it.each([
    [new Date(2026, 9, 20, 10, 12), '今天 10:12'],
    [new Date(2026, 9, 19, 21, 40), '昨天 21:40'],
    [new Date(2026, 9, 16, 15, 2), '10月16日'],
    [new Date(2025, 9, 16), '2025-10-16'],
  ])('formats %s in local time', (date, expected) => {
    expect(formatWhen(micros(date), now.getTime())).toBe(expected);
  });

  it('recognizes yesterday across a year boundary', () => {
    const newYear = new Date(2026, 0, 1, 9, 0);
    expect(formatWhen(micros(new Date(2025, 11, 31, 23, 0)), newYear.getTime())).toBe('昨天 23:00');
  });

  it('renders nullish timestamps as an empty string', () => {
    expect(formatWhen(null, now.getTime())).toBe('');
    expect(formatWhen(undefined, now.getTime())).toBe('');
  });
});

describe('date and duration formatting', () => {
  it('formats local date and time', () => {
    expect(formatDateTime(micros(new Date(2026, 8, 28, 21, 14)))).toBe('2026-09-28 21:14');
  });

  it('formats durations in seconds', () => {
    expect(formatDurationS(2538)).toBe('42:18');
    expect(formatDurationS(null)).toBe('');
    expect(formatDurationS(undefined)).toBe('');
  });
});
