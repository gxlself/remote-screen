export const colors = {
  bg: '#0d1114',
  card: '#151a1e',
  cardLine: '#232b31',
  raised: '#1c2328',
  selected: '#2e3840',
  text: '#ecf0f2',
  text2: '#a9b4bb',
  text3: '#86929a',
  accent: '#7da8ff',
  onAccent: '#0a1630',
  good: '#5dd39a',
  relay: '#f4b24e',
  bad: '#ff8873',
  hostControlled: '#ff9f43',
  winAccent: '#4cc2ff',
  onControlled: '#2a1500',
  windowBg: '#1e2226',
  titleBar: '#262b30',
  sidebar: '#191d21',
  listPane: '#1b1f23',
  line: '#2a3036',
  line2: '#2e343a',
  menuBg: 'rgba(30, 34, 38, 0.94)',
  dialogBg: 'rgba(32, 36, 40, 0.96)',
  macBlue: '#3d7bea',
  textSoft: '#c2cbd1',
  textStrong: '#d5dce0',
  muted: '#4e5a63',
  switchOff: '#4a545c',
  iconTile: '#2a3036',
  accentSoft: '#a9c3ff',
  dangerSoft: '#ffb3a6',
  dangerBg: 'rgba(255, 136, 115, 0.2)',
  controlledBg: 'rgba(255, 159, 67, 0.12)',
  controlledLine: 'rgba(255, 159, 67, 0.35)',
} as const;

export const radii = {
  menu: 12,
  card: 10,
  button: 7,
  row: 6,
  pill: 18,
} as const;

export const font = {
  sans: "'Geist Variable', 'Geist', 'PingFang SC', 'Hiragino Sans GB', system-ui, sans-serif",
  mono: "'Geist Mono Variable', 'Geist Mono', ui-monospace, monospace",
} as const;

export const type = {
  base: 13,
  small: 11.5,
} as const;

export const tokens = { colors, radii, font, type } as const;
