import { PixelRatio } from 'react-native';
import {
  ACCESSIBILITY_FONT_SCALE,
  boxHeight,
  isAccessibilityFontScale,
} from './fontScale';

describe('accessibility font scale', () => {
  afterEach(() => jest.restoreAllMocks());

  it('keeps the threshold separate from the Home layout threshold', () => {
    expect(ACCESSIBILITY_FONT_SCALE).toBe(2);
  });

  it.each([1, 2])('keeps a fixed height at scale %s', scale => {
    expect(isAccessibilityFontScale(scale)).toBe(false);
    expect(boxHeight(44, scale)).toEqual({ height: 44 });
  });

  it('allows the box to grow at scale 3', () => {
    expect(isAccessibilityFontScale(3)).toBe(true);
    expect(boxHeight(44, 3)).toEqual({ minHeight: 44 });
  });

  it('starts growing immediately above the threshold', () => {
    expect(isAccessibilityFontScale(2.01)).toBe(true);
  });

  it('reads PixelRatio when no explicit scale is supplied', () => {
    const getFontScale = jest
      .spyOn(PixelRatio, 'getFontScale')
      .mockReturnValue(3);
    expect(isAccessibilityFontScale()).toBe(true);
    expect(boxHeight(46)).toEqual({ minHeight: 46 });
    expect(getFontScale).toHaveBeenCalledTimes(2);

    getFontScale.mockReturnValue(2);
    expect(boxHeight(46)).toEqual({ height: 46 });
  });
});
