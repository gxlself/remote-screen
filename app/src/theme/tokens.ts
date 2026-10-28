import { Platform } from 'react-native';

export const tokens = {
  bg: '#0D1114',
  text: '#ECF0F2',
  card: '#151A1E',
  text2: '#A9B4BB',
  cardLine: '#232B31',
  text3: '#86929A',
  raised: '#1C2328',
  accent: '#7DA8FF',
  accentText: '#0A1630',
  selected: '#2E3840',
  good: '#5DD39A',
  relay: '#F4B24E',
  bad: '#FF8873',
  hostControlled: '#FF9F43',
  winAccent: '#4CC2FF',
  track: '#252D33',
  switchOff: '#3A444B',
  muted: '#4E5A63',
  inputLine: '#2A333A',
  // Text field outline: at least 3:1 on the card and the canvas.
  inputBorder: '#64707A',
  offlineCard: '#12171A',
  thumbBg: '#0A0D10',
  hud: 'rgba(10, 13, 16, 0.8)',
  hudLine: 'rgba(255, 255, 255, 0.08)',
  toolbar: 'rgba(13, 17, 20, 0.86)',
  panel: 'rgba(21, 26, 30, 0.97)',
  panelLine: '#262E34',
  accentSoft: 'rgba(125, 168, 255, 0.16)',
  accentTint: 'rgba(125, 168, 255, 0.08)',
  goodSoft: 'rgba(93, 211, 154, 0.14)',
  relaySoft: 'rgba(244, 178, 78, 0.14)',
  badSoft: 'rgba(255, 136, 115, 0.16)',
  onHostControlled: '#2A1500',
  onRelay: '#2A1A00',
  knob: '#FFFFFF',
} as const;

export type ThemeTokens = typeof tokens;

export type ThemeTokenName = keyof typeof tokens;

export const radius = {
  segment: 9,
  button: 12,
  thumb: 10,
  card: 16,
  panel: 22,
  toolbar: 28,
  pill: 999,
} as const;

export const space = {
  xs: 4,
  sm: 8,
  md: 12,
  lg: 16,
  gutter: 20,
  section: 24,
} as const;

export const hit = {
  get min(): number {
    return Platform.OS === 'android' ? 48 : 44;
  },
  landscapeSafe: 59,
} as const;
