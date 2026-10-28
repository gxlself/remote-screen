import { Platform } from 'react-native';

export function isAndroid(): boolean {
  return Platform.OS === 'android';
}

export const material = {
  topBar: 64,
  navBar: 80,
  navIndicator: { width: 64, height: 32 },
  switchTrack: { width: 52, height: 32 },
  // M3 specification values; the Android table only gives the track size.
  switchThumbOff: 16,
  switchThumbOn: 24,
  dialogRadius: 28,
  // M3 specification values not specified in the Android table.
  sheetRadius: 28,
  snackbarRadius: 4,
  minTouch: 48,
} as const;

// The Android table does not specify ripple colors; use the task defaults.
export const ripple = {
  neutral: 'rgba(255, 255, 255, 0.12)',
  accent: 'rgba(125, 168, 255, 0.24)',
  danger: 'rgba(255, 136, 115, 0.24)',
} as const;

// iOS has no ripple: pressed controls dim, rows and segments gain a highlight.
export const press = {
  opacity: 0.6,
  highlight: 'rgba(255, 255, 255, 0.08)',
} as const;
