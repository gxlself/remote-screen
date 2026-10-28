import { Platform } from 'react-native';
import { hit, radius, space, tokens } from './tokens';

describe('theme tokens', () => {
  it.each([
    ['bg', '#0D1114'],
    ['text', '#ECF0F2'],
    ['card', '#151A1E'],
    ['text2', '#A9B4BB'],
    ['cardLine', '#232B31'],
    ['text3', '#86929A'],
    ['raised', '#1C2328'],
    ['accent', '#7DA8FF'],
    ['accentText', '#0A1630'],
    ['selected', '#2E3840'],
    ['good', '#5DD39A'],
    ['relay', '#F4B24E'],
    ['bad', '#FF8873'],
    ['hostControlled', '#FF9F43'],
    ['winAccent', '#4CC2FF'],
  ] as const)('keeps the P1 %s color', (name, value) => {
    expect(tokens[name]).toBe(value);
  });

  const added = [
    'track',
    'switchOff',
    'muted',
    'inputLine',
    'inputBorder',
    'offlineCard',
    'thumbBg',
    'hud',
    'hudLine',
    'toolbar',
    'panel',
    'panelLine',
    'accentSoft',
    'accentTint',
    'goodSoft',
    'relaySoft',
    'badSoft',
    'onHostControlled',
    'onRelay',
    'knob',
  ] as const;

  it.each(added)('defines a valid color for %s', name => {
    expect(tokens[name]).toMatch(/^(#[0-9A-F]{6}|rgba\([^)]+\))$/);
  });

  it('exports layout constants', () => {
    expect(radius).toEqual({
      segment: 9,
      button: 12,
      thumb: 10,
      card: 16,
      panel: 22,
      toolbar: 28,
      pill: 999,
    });
    expect(space).toEqual({
      xs: 4,
      sm: 8,
      md: 12,
      lg: 16,
      gutter: 20,
      section: 24,
    });
    expect(hit).toEqual({ min: 44, landscapeSafe: 59 });
    expect(radius.card).toBe(16);
    expect(hit.min).toBe(44);
  });
});

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map(i => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

describe('input border token', () => {
  it.each(['card', 'bg'] as const)(
    'keeps at least 3:1 against %s',
    background => {
      expect(
        contrast(tokens.inputBorder, tokens[background]),
      ).toBeGreaterThanOrEqual(3);
    },
  );
});

describe('platform touch targets', () => {
  afterEach(() => jest.restoreAllMocks());

  it('reads the platform on each access without changing landscape clearance', () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    expect(hit.min).toBe(48);
    expect(hit.landscapeSafe).toBe(59);

    jest.replaceProperty(Platform, 'OS', 'ios');
    expect(hit.min).toBe(44);
  });
});
