export type Shape =
  | { t: 'path'; d: string }
  | { t: 'rect'; x: number; y: number; w: number; h: number; rx: number }
  | { t: 'circle'; cx: number; cy: number; r: number; filled?: boolean }
  | { t: 'ellipse'; cx: number; cy: number; rx: number; ry: number };

export const iconShapes = {
  alert: [
    { t: 'circle', cx: 12, cy: 12, r: 9 },
    { t: 'path', d: 'M12 7.5v5.5M12 16.5h.01' },
  ],
  arrowRight: [{ t: 'path', d: 'M5 12h14M13 6l6 6-6 6' }],
  arrowUp: [{ t: 'path', d: 'M12 19V5M6 11l6-6 6 6' }],
  bell: [
    {
      t: 'path',
      d: 'M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15z',
    },
    { t: 'path', d: 'M10 20.5a2 2 0 0 0 4 0' },
  ],
  blocked: [
    { t: 'circle', cx: 12, cy: 12, r: 8.5 },
    { t: 'path', d: 'M6 6l12 12' },
  ],
  bolt: [{ t: 'path', d: 'M13 3L5 14h6l-1 7 8-11h-6z' }],
  check: [{ t: 'path', d: 'M5 12.5l4.5 4.5L19 7.5' }],
  chevronDown: [{ t: 'path', d: 'M6 9.5l6 6 6-6' }],
  chevronLeft: [{ t: 'path', d: 'M14.5 5.5L8 12l6.5 6.5' }],
  chevronRight: [{ t: 'path', d: 'M9.5 5.5L16 12l-6.5 6.5' }],
  chip: [
    { t: 'rect', x: 7, y: 7, w: 10, h: 10, rx: 2 },
    {
      t: 'path',
      d: 'M9.5 3v4M14.5 3v4M9.5 17v4M14.5 17v4M3 9.5h4M3 14.5h4M17 9.5h4M17 14.5h4',
    },
  ],
  clipboard: [
    { t: 'rect', x: 5, y: 4.5, w: 14, h: 17, rx: 2 },
    { t: 'path', d: 'M9 4.5V3.5h6v1M9 11h6M9 15h4' },
  ],
  clock: [
    { t: 'circle', cx: 12, cy: 12, r: 8.5 },
    { t: 'path', d: 'M12 7.5V12l3 2' },
  ],
  close: [{ t: 'path', d: 'M6.5 6.5l11 11M17.5 6.5l-11 11' }],
  computer: [
    { t: 'rect', x: 3, y: 4, w: 18, h: 12.5, rx: 2 },
    { t: 'path', d: 'M9 20h6M12 16.5V20' },
  ],
  computerAdd: [
    { t: 'rect', x: 3, y: 4, w: 18, h: 12.5, rx: 2 },
    { t: 'path', d: 'M9 20h6M12 16.5V20M9.5 10.25h5M12 7.75v5' },
  ],
  computerOff: [
    { t: 'rect', x: 3, y: 4, w: 18, h: 12.5, rx: 2 },
    { t: 'path', d: 'M9 20h6M12 16.5V20M3 3l18 18' },
  ],
  copy: [
    { t: 'rect', x: 8, y: 8, w: 12, h: 12, rx: 2.5 },
    {
      t: 'path',
      d: 'M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2',
    },
  ],
  database: [
    { t: 'ellipse', cx: 12, cy: 5.5, rx: 7.5, ry: 2.5 },
    {
      t: 'path',
      d: 'M4.5 5.5v6.5c0 1.4 3.4 2.5 7.5 2.5s7.5-1.1 7.5-2.5V5.5M4.5 12v6.5c0 1.4 3.4 2.5 7.5 2.5s7.5-1.1 7.5-2.5V12',
    },
  ],
  displays: [
    { t: 'rect', x: 2, y: 5, w: 13, h: 9, rx: 1.5 },
    { t: 'rect', x: 17, y: 3, w: 5, h: 12, rx: 1.2 },
    { t: 'path', d: 'M6 18h5' },
  ],
  download: [{ t: 'path', d: 'M12 4v11M7.5 10.5L12 15l4.5-4.5M5 19.5h14' }],
  dpad: [
    {
      t: 'path',
      d: 'M12 3.5l2.6 3.2H9.4zM12 20.5l-2.6-3.2h5.2zM3.5 12l3.2-2.6v5.2zM20.5 12l-3.2 2.6V9.4z',
    },
    { t: 'circle', cx: 12, cy: 12, r: 1.6 },
  ],
  external: [
    {
      t: 'path',
      d: 'M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4',
    },
  ],
  eye: [
    {
      t: 'path',
      d: 'M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z',
    },
    { t: 'circle', cx: 12, cy: 12, r: 3 },
  ],
  eyeOff: [
    {
      t: 'path',
      d: 'M3 3l18 18M10.6 5.6A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.6 3.4M6.2 7.4A15.6 15.6 0 0 0 2.5 12S6 18.5 12 18.5a9 9 0 0 0 4-.9',
    },
  ],
  faceId: [
    {
      t: 'path',
      d: 'M4 8.5v-2A2.5 2.5 0 0 1 6.5 4h2M15.5 4h2A2.5 2.5 0 0 1 20 6.5v2M20 15.5v2a2.5 2.5 0 0 1-2.5 2.5h-2M8.5 20h-2A2.5 2.5 0 0 1 4 17.5v-2M9 9.5V11M15 9.5V11M12.2 9.5v3.6h-.9M9.3 15.6c1.6 1.3 3.8 1.3 5.4 0',
    },
  ],
  file: [
    { t: 'path', d: 'M6 3h8l4 4v14H6z' },
    { t: 'path', d: 'M9 12h6M9 16h6' },
  ],
  fileUpload: [
    {
      t: 'path',
      d: 'M14 3.5H7A1.5 1.5 0 0 0 5.5 5v14A1.5 1.5 0 0 0 7 20.5h10a1.5 1.5 0 0 0 1.5-1.5V8z',
    },
    { t: 'path', d: 'M14 3.5V8h4.5M12 17v-6M9.5 13.5L12 11l2.5 2.5' },
  ],
  flashlight: [
    {
      t: 'path',
      d: 'M8 3h8l-1.5 5h-5zM9.5 8h5v12.5a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1z',
    },
    { t: 'path', d: 'M12 12v3' },
  ],
  folder: [
    {
      t: 'path',
      d: 'M3 7.5a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
    },
  ],
  forceQuit: [
    { t: 'circle', cx: 12, cy: 12, r: 8.5 },
    { t: 'path', d: 'M9 9l6 6M15 9l-6 6' },
  ],
  globe: [
    { t: 'circle', cx: 12, cy: 12, r: 9 },
    {
      t: 'path',
      d: 'M3 12h18M12 3c2.6 2.6 3.8 5.6 3.8 9s-1.2 6.4-3.8 9M12 3C9.4 5.6 8.2 8.6 8.2 12s1.2 6.4 3.8 9',
    },
  ],
  hand: [
    {
      t: 'path',
      d: 'M8 13V6.5a1.5 1.5 0 0 1 3 0V12M11 11V5a1.5 1.5 0 0 1 3 0v6M14 11V6.5a1.5 1.5 0 0 1 3 0V14c0 4-2.5 6.5-6 6.5-2.5 0-4-1.2-5.2-3.4L4 13.6a1.4 1.4 0 0 1 2.3-1.6L8 14',
    },
  ],
  info: [
    { t: 'circle', cx: 12, cy: 12, r: 9 },
    { t: 'path', d: 'M12 11v5M12 8h.01' },
  ],
  key: [
    { t: 'circle', cx: 8, cy: 15, r: 4 },
    { t: 'path', d: 'M11 12l8.5-8.5M16.5 6.5l2.5 2.5M14 9l2 2' },
  ],
  keyboard: [
    { t: 'rect', x: 2.5, y: 6, w: 19, h: 12, rx: 2.5 },
    { t: 'path', d: 'M6.5 10h.01M10 10h.01M14 10h.01M17.5 10h.01M8 14h8' },
  ],
  keyboardHide: [
    { t: 'rect', x: 2.5, y: 3.5, w: 19, h: 11, rx: 2.5 },
    {
      t: 'path',
      d: 'M6.5 7.5h.01M10 7.5h.01M14 7.5h.01M17.5 7.5h.01M8 11h8M9 18.5l3 2.5 3-2.5',
    },
  ],
  laptop: [
    {
      t: 'path',
      d: 'M5 6.5A1.5 1.5 0 0 1 6.5 5h11A1.5 1.5 0 0 1 19 6.5V16H5z',
    },
    {
      t: 'path',
      d: 'M2.5 16h19l-.8 2.2a1.2 1.2 0 0 1-1.1.8H4.4a1.2 1.2 0 0 1-1.1-.8z',
    },
  ],
  lock: [
    { t: 'rect', x: 5, y: 10.5, w: 14, h: 10, rx: 2 },
    { t: 'path', d: 'M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5' },
  ],
  moon: [
    { t: 'path', d: 'M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z' },
  ],
  more: [
    { t: 'circle', cx: 5.5, cy: 12, r: 1.8 },
    { t: 'circle', cx: 12, cy: 12, r: 1.8 },
    { t: 'circle', cx: 18.5, cy: 12, r: 1.8 },
  ],
  paste: [
    { t: 'rect', x: 5, y: 4.5, w: 14, h: 17, rx: 2 },
    { t: 'path', d: 'M9 4.5V3.5h6v1M12 9.5v7M9 13.5l3 3 3-3' },
  ],
  pause: [
    { t: 'rect', x: 6, y: 5, w: 4, h: 14, rx: 1 },
    { t: 'rect', x: 14, y: 5, w: 4, h: 14, rx: 1 },
  ],
  phone: [
    { t: 'rect', x: 6.5, y: 2.5, w: 11, h: 19, rx: 2.5 },
    { t: 'path', d: 'M10.5 18.5h3' },
  ],
  play: [{ t: 'path', d: 'M8 5.5v13l10.5-6.5z' }],
  plus: [{ t: 'path', d: 'M12 5v14M5 12h14' }],
  pointer: [
    { t: 'path', d: 'M6 3.5l11.5 9.2-5.2.6 3 6.2-2.4 1.1-3-6.3L6 18z' },
  ],
  pointerPause: [
    { t: 'path', d: 'M5 3.5v14.5l4-3.8 2.8 6.3 2.6-1.1-2.7-6.2H17z' },
    { t: 'path', d: 'M19.5 4v5M22 4v5' },
  ],
  power: [
    { t: 'path', d: 'M12 3.5v8' },
    { t: 'path', d: 'M7 6.8a7.3 7.3 0 1 0 10 0' },
  ],
  prompt: [{ t: 'path', d: 'M5 8l4 4-4 4M12 16h7' }],
  quality: [
    { t: 'path', d: 'M4 7h10M18 7h2M4 17h4M12 17h8' },
    { t: 'circle', cx: 16, cy: 7, r: 2.2 },
    { t: 'circle', cx: 10, cy: 17, r: 2.2 },
  ],
  record: [{ t: 'circle', cx: 12, cy: 12, r: 6.5, filled: true }],
  refresh: [
    { t: 'path', d: 'M4 12a8 8 0 0 1 13.7-5.6M20 12a8 8 0 0 1-13.7 5.6' },
    { t: 'path', d: 'M18 3v4h-4M6 21v-4h4' },
  ],
  relay: [{ t: 'path', d: 'M4 12h4l3-7 3 14 3-7h3' }],
  retry: [
    { t: 'path', d: 'M20 12a8 8 0 1 1-2.4-5.7' },
    { t: 'path', d: 'M20 4.5v4.5h-4.5' },
  ],
  scanCode: [
    {
      t: 'path',
      d: 'M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3M8 12h8',
    },
  ],
  screenshot: [
    {
      t: 'path',
      d: 'M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16',
    },
    { t: 'circle', cx: 12, cy: 12, r: 3 },
  ],
  search: [
    { t: 'circle', cx: 10.5, cy: 10.5, r: 6 },
    { t: 'path', d: 'M15 15l5 5' },
  ],
  sessions: [
    { t: 'path', d: 'M12 4l9 4.5-9 4.5-9-4.5z' },
    { t: 'path', d: 'M3 13l9 4.5 9-4.5' },
  ],
  share: [
    {
      t: 'path',
      d: 'M12 15V3.5M7.5 8L12 3.5 16.5 8M6 11H5a1 1 0 0 0-1 1v7.5a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1V12a1 1 0 0 0-1-1h-1',
    },
  ],
  shield: [
    {
      t: 'path',
      d: 'M12 3l7.5 3v5.5c0 4.6-3.2 8.3-7.5 9.5-4.3-1.2-7.5-4.9-7.5-9.5V6z',
    },
  ],
  shieldCheck: [
    {
      t: 'path',
      d: 'M12 3l7.5 3v5.5c0 4.6-3.2 8.3-7.5 9.5-4.3-1.2-7.5-4.9-7.5-9.5V6z',
    },
    { t: 'path', d: 'M8.8 12.2l2.2 2.2 4.2-4.4' },
  ],
  shieldKey: [
    {
      t: 'path',
      d: 'M12 3l7 3v5c0 4.5-3 8.2-7 10-4-1.8-7-5.5-7-10V6z',
    },
    { t: 'circle', cx: 12, cy: 10.5, r: 1.8 },
    { t: 'path', d: 'M12 12.3v3.4' },
  ],
  spinner: [{ t: 'path', d: 'M12 3a9 9 0 1 1-9 9' }],
  stop: [{ t: 'rect', x: 5, y: 5, w: 14, h: 14, rx: 2.5 }],
  swipeHand: [
    {
      t: 'path',
      d: 'M7 13V6.5a1.5 1.5 0 0 1 3 0V12M10 11V5a1.5 1.5 0 0 1 3 0v6M13 11V6a1.5 1.5 0 0 1 3 0v8c0 4-2.5 6.5-6 6.5-2.5 0-4-1.2-5.2-3.4L3 13.6a1.4 1.4 0 0 1 2.3-1.6L7 14',
    },
    { t: 'path', d: 'M18.5 8.5l2 2-2 2M20.5 10.5h-2' },
  ],
  tablet: [
    { t: 'rect', x: 4, y: 2.5, w: 16, h: 19, rx: 2.5 },
    { t: 'path', d: 'M10.5 18.5h3' },
  ],
  terminal: [
    { t: 'rect', x: 2.5, y: 4, w: 19, h: 16, rx: 2.5 },
    { t: 'path', d: 'M7 9.5l3 2.5-3 2.5M12.5 15h4.5' },
  ],
  trash: [
    { t: 'path', d: 'M4.5 7h15M9.5 7V4.5h5V7M6.5 7l1 13h9l1-13' },
    { t: 'path', d: 'M10 11v5.5M14 11v5.5' },
  ],
  uploadTo: [{ t: 'path', d: 'M12 19V8M7 12l5-5 5 5M5 4h14' }],
  user: [
    { t: 'circle', cx: 12, cy: 8.5, r: 3.5 },
    { t: 'path', d: 'M5 20c.8-3.6 3.6-5.5 7-5.5s6.2 1.9 7 5.5' },
  ],
  userLock: [
    { t: 'circle', cx: 9, cy: 8, r: 3.5 },
    { t: 'path', d: 'M3 19.5a6 6 0 0 1 10.5-4' },
    { t: 'rect', x: 14, y: 14, w: 7, h: 6, rx: 1.2 },
    { t: 'path', d: 'M15.5 14v-1.5a2 2 0 0 1 4 0V14' },
  ],
  vpn: [
    { t: 'path', d: 'M3.5 19.5V12a8.5 8.5 0 0 1 17 0v7.5z' },
    { t: 'path', d: 'M8.5 19.5V13a3.5 3.5 0 0 1 7 0v6.5' },
  ],
  warning: [
    { t: 'path', d: 'M12 3.5l9.5 16.5h-19z' },
    { t: 'path', d: 'M12 10v4.5M12 17.2h.01' },
  ],
  wifi: [
    {
      t: 'path',
      d: 'M2.5 9a14 14 0 0 1 19 0M5.5 12.5a9.5 9.5 0 0 1 13 0M8.6 16a5 5 0 0 1 6.8 0',
    },
    { t: 'circle', cx: 12, cy: 19.4, r: 1, filled: true },
  ],
  zoomIn: [
    { t: 'circle', cx: 11, cy: 11, r: 6.5 },
    { t: 'path', d: 'M16 16l4.5 4.5M11 8.5v5M8.5 11h5' },
  ],
} as const satisfies Record<string, readonly Shape[]>;

export type IconName = keyof typeof iconShapes;
