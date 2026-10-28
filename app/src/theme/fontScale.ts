import { PixelRatio } from 'react-native';

export const ACCESSIBILITY_FONT_SCALE = 2;

export function isAccessibilityFontScale(
  scale = PixelRatio.getFontScale(),
): boolean {
  return scale > ACCESSIBILITY_FONT_SCALE;
}

export function boxHeight(
  height: number,
  scale?: number,
): { height: number } | { minHeight: number } {
  return isAccessibilityFontScale(scale) ? { minHeight: height } : { height };
}
