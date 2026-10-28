import { fontFor, textStyle } from './typography';

describe('typography', () => {
  it('maps sans weights to PostScript font names', () => {
    expect(fontFor(400)).toBe('Geist-Regular');
    expect(fontFor(600)).toBe('Geist-SemiBold');
  });

  it('uses the mono family and falls back to 600 for mono 700', () => {
    expect(fontFor(500, true)).toBe('GeistMono-Medium');
    expect(fontFor(700, true)).toBe('GeistMono-SemiBold');
  });

  it('returns body defaults', () => {
    expect(textStyle('body')).toEqual({
      fontSize: 15,
      lineHeight: 22,
      fontFamily: 'Geist-Regular',
    });
  });

  it('applies a weight override', () => {
    expect(textStyle('caption', 600)).toEqual({
      fontSize: 12,
      lineHeight: 17,
      fontFamily: 'Geist-SemiBold',
    });
  });

  it('adds tracking to section headings', () => {
    expect(textStyle('section')).toEqual({
      fontSize: 13,
      lineHeight: 19,
      fontFamily: 'Geist-SemiBold',
      letterSpacing: 0.52,
    });
  });

  it('uses mono fonts for mono variants', () => {
    expect(textStyle('mono').fontFamily).toBe('GeistMono-Regular');
    expect(textStyle('monoSmall').fontFamily).toBe('GeistMono-Regular');
  });
});
