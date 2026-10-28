import { Platform } from 'react-native';
import { isAndroid, material, press, ripple } from './platform';

describe('platform theme', () => {
  afterEach(() => jest.restoreAllMocks());

  it('reads the current platform on every call', () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    expect(isAndroid()).toBe(false);
    jest.replaceProperty(Platform, 'OS', 'android');
    expect(isAndroid()).toBe(true);
    jest.replaceProperty(Platform, 'OS', 'ios');
    expect(isAndroid()).toBe(false);
  });

  it('matches the Android table in 07-app', () => {
    expect(material).toMatchObject({
      topBar: 64,
      navBar: 80,
      navIndicator: { width: 64, height: 32 },
      switchTrack: { width: 52, height: 32 },
      dialogRadius: 28,
      minTouch: 48,
    });
  });

  it('exports the remaining M3 specification values', () => {
    expect(material).toMatchObject({
      switchThumbOff: 16,
      switchThumbOn: 24,
      sheetRadius: 28,
      snackbarRadius: 4,
    });
  });

  it('uses the ripple colors specified by the task', () => {
    expect(ripple).toEqual({
      neutral: 'rgba(255, 255, 255, 0.12)',
      accent: 'rgba(125, 168, 255, 0.24)',
      danger: 'rgba(255, 136, 115, 0.24)',
    });
  });

  it('defines the iOS pressed feedback', () => {
    expect(press).toEqual({
      opacity: 0.6,
      highlight: 'rgba(255, 255, 255, 0.08)',
    });
  });
});
