import {
  activeModifierPrefix,
  keyboardStrings,
  type KeyboardOS,
} from './strings';

export type ModKey = 'cmd' | 'opt' | 'ctl' | 'shf';
export type ModState = 'none' | 'once' | 'lock';
export type ModStates = Record<ModKey, ModState>;
export type InputMode = 'text' | 'keys';
export type Arrow = 'left' | 'right' | 'up' | 'down';

export type KeyEvent = { usage: number; down: boolean; mods: number };

export type ShortcutId =
  | 'cmdC'
  | 'cmdV'
  | 'cmdZ'
  | 'cmdSpace'
  | 'cmdTab'
  | 'cmdW'
  | 'cmdQ'
  | 'ctrlC'
  | 'ctrlV'
  | 'altTab'
  | 'winD'
  | 'ctrlAltDel'
  | 'ctrlShiftEsc'
  | 'altF4'
  | 'win'
  | 'pasteAsTyping';

export type ShortcutPlan =
  | { kind: 'keys'; events: KeyEvent[] }
  | {
      kind: 'combo';
      combo: 'cmdTab' | 'cmdSpace' | 'missionControl' | 'altTab' | 'winKey';
    }
  | { kind: 'action'; action: 'ctrlAltDel' }
  | { kind: 'pasteAsTyping' };

export const KEY_ESCAPE = 0x29;
export const KEY_TAB = 0x2b;
export const KEY_ENTER = 0x28;
export const KEY_BACKSPACE = 0x2a;
export const KEY_SPACE = 0x2c;
export const KEY_DELETE = 0x4c;
export const KEY_HOME = 0x4a;
export const KEY_PAGE_UP = 0x4b;
export const KEY_END = 0x4d;
export const KEY_PAGE_DOWN = 0x4e;
export const KEY_F1 = 0x3a;
export const KEY_F12 = 0x45;
export const KEY_ARROW_RIGHT = 0x4f;
export const KEY_ARROW_LEFT = 0x50;
export const KEY_ARROW_DOWN = 0x51;
export const KEY_ARROW_UP = 0x52;
export const KEY_A = 0x04;
export const KEY_C = 0x06;
export const KEY_D = 0x07;
export const KEY_V = 0x19;
export const KEY_Z = 0x1d;
export const KEY_Q = 0x14;
export const KEY_W = 0x1a;
export const KEY_F4 = 0x3d;

export const KEY_CTRL = 0xe0;
export const KEY_SHIFT = 0xe1;
export const KEY_ALT = 0xe2;
export const KEY_GUI = 0xe3;

export const MOD_CTRL = 0x01;
export const MOD_SHIFT = 0x02;
export const MOD_ALT = 0x04;
export const MOD_GUI = 0x08;

export const ARROW_STEP_PT = 24;

export const NO_MODS: ModStates = {
  cmd: 'none',
  opt: 'none',
  ctl: 'none',
  shf: 'none',
};

const modifierSequence = [
  { key: 'ctl' as const, bit: MOD_CTRL, usage: KEY_CTRL },
  { key: 'shf' as const, bit: MOD_SHIFT, usage: KEY_SHIFT },
  { key: 'opt' as const, bit: MOD_ALT, usage: KEY_ALT },
  { key: 'cmd' as const, bit: MOD_GUI, usage: KEY_GUI },
];

const functionKeyUsages = [
  0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45,
] as const;

const arrowUsages: Record<Arrow, number> = {
  right: KEY_ARROW_RIGHT,
  left: KEY_ARROW_LEFT,
  down: KEY_ARROW_DOWN,
  up: KEY_ARROW_UP,
};

export function cycle(state: ModState): ModState {
  switch (state) {
    case 'none':
      return 'once';
    case 'once':
      return 'lock';
    case 'lock':
      return 'none';
  }
}

export function modsMask(state: ModStates): number {
  let mask = 0;
  if (state.ctl !== 'none') {
    mask += MOD_CTRL;
  }
  if (state.shf !== 'none') {
    mask += MOD_SHIFT;
  }
  if (state.opt !== 'none') {
    mask += MOD_ALT;
  }
  if (state.cmd !== 'none') {
    mask += MOD_GUI;
  }
  return mask;
}

export function isModifierActive(state: ModStates): boolean {
  return (Object.keys(state) as ModKey[]).some(key => state[key] !== 'none');
}

export function mergeMods(mods: number, extra: number): number {
  let merged = mods % 16;
  for (const bit of [MOD_CTRL, MOD_SHIFT, MOD_ALT, MOD_GUI]) {
    if (hasMask(extra, bit) && !hasMask(merged, bit)) {
      merged += bit;
    }
  }
  return merged;
}

export function consumeOnce(state: ModStates): ModStates {
  let changed = false;
  const next: ModStates = { ...state };
  for (const key of Object.keys(next) as ModKey[]) {
    if (next[key] === 'once') {
      next[key] = 'none';
      changed = true;
    }
  }
  return changed ? next : state;
}

export function keyPress(usage: number, mods: number): KeyEvent[] {
  const wanted = mods % 16;
  const events: KeyEvent[] = [];
  let current = 0;

  for (const modifier of modifierSequence) {
    if (!hasMask(wanted, modifier.bit)) {
      continue;
    }
    current += modifier.bit;
    events.push({ usage: modifier.usage, down: true, mods: current });
  }

  events.push({ usage, down: true, mods: current });
  events.push({ usage, down: false, mods: current });

  for (const modifier of [...modifierSequence].reverse()) {
    if (!hasMask(wanted, modifier.bit)) {
      continue;
    }
    current -= modifier.bit;
    events.push({ usage: modifier.usage, down: false, mods: current });
  }

  return events;
}

function hasMask(mask: number, bit: number): boolean {
  return Math.floor(mask / bit) % 2 === 1;
}

export function functionKeyUsage(index: number): number {
  return functionKeyUsages[Math.min(12, Math.max(1, index)) - 1];
}

export function arrowUsage(arrow: Arrow): number {
  return arrowUsages[arrow];
}

export type NavKey = 'home' | 'end' | 'pageUp' | 'pageDown';
export const NAV_KEYS: readonly NavKey[] = ['home', 'end', 'pageUp', 'pageDown'];

const navUsages: Record<NavKey, number> = {
  home: KEY_HOME,
  end: KEY_END,
  pageUp: KEY_PAGE_UP,
  pageDown: KEY_PAGE_DOWN,
};

export function navKeyUsage(key: NavKey): number {
  return navUsages[key];
}

/**
 * Cmd+V（Windows 上是映射后的 Ctrl+V）的完整按键序列，剪贴板推完以后发。
 * `held` 是外接键盘上按着的修饰键（已按电脑系统映射）。和原生
 * KeyboardPassthrough.keyEvents 一样：按着的是右侧键就用右侧键，发完再把
 * 按着的那只键按回去，之后物理松开时电脑上的状态才对得上。
 */
export function pasteKeys(os: KeyboardOS, held: number): KeyEvent[] {
  const left = os === 'windows' ? MOD_CTRL : MOD_GUI;
  const right = left * 16;
  const useRight = hasMask(held, right);
  const bit = useRight ? right : left;
  const usage = (os === 'windows' ? KEY_CTRL : KEY_GUI) + (useRight ? 4 : 0);
  const holding = hasMask(held, bit);
  const mods = holding ? held : held + bit;
  const events: KeyEvent[] = [
    { usage, down: true, mods },
    { usage: KEY_V, down: true, mods },
    { usage: KEY_V, down: false, mods },
    { usage, down: false, mods: mods - bit },
  ];
  if (holding) {
    events.push({ usage, down: true, mods: held });
  }
  return events;
}

export function isModifierUsage(usage: number): boolean {
  return usage >= KEY_CTRL && usage <= KEY_GUI + 7;
}

// The shortcut id selects the platform-specific binding; `os` is accepted so
// callers can construct plans for a host without maintaining a second table.
export function shortcut(id: ShortcutId, _os: KeyboardOS): ShortcutPlan {
  switch (id) {
    case 'cmdC':
      return { kind: 'keys', events: keyPress(KEY_C, MOD_GUI) };
    case 'cmdV':
      return { kind: 'keys', events: keyPress(KEY_V, MOD_GUI) };
    case 'cmdZ':
      return { kind: 'keys', events: keyPress(KEY_Z, MOD_GUI) };
    case 'cmdSpace':
      return { kind: 'combo', combo: 'cmdSpace' };
    case 'cmdTab':
      return { kind: 'combo', combo: 'cmdTab' };
    case 'cmdW':
      return { kind: 'keys', events: keyPress(KEY_W, MOD_GUI) };
    case 'cmdQ':
      return { kind: 'keys', events: keyPress(KEY_Q, MOD_GUI) };
    case 'ctrlC':
      return { kind: 'keys', events: keyPress(KEY_C, MOD_CTRL) };
    case 'ctrlV':
      return { kind: 'keys', events: keyPress(KEY_V, MOD_CTRL) };
    case 'altTab':
      return { kind: 'combo', combo: 'altTab' };
    case 'winD':
      return { kind: 'keys', events: keyPress(KEY_D, MOD_GUI) };
    case 'ctrlAltDel':
      return { kind: 'action', action: 'ctrlAltDel' };
    case 'ctrlShiftEsc':
      return {
        kind: 'keys',
        events: keyPress(KEY_ESCAPE, MOD_CTRL + MOD_SHIFT),
      };
    case 'altF4':
      return { kind: 'keys', events: keyPress(KEY_F4, MOD_ALT) };
    case 'win':
      return { kind: 'combo', combo: 'winKey' };
    case 'pasteAsTyping':
      return { kind: 'pasteAsTyping' };
  }
}

export class ArrowTracker {
  private x = 0;
  private y = 0;
  private direction: Arrow | null = null;

  constructor(private readonly step = ARROW_STEP_PT) {}

  start(): void {
    this.x = 0;
    this.y = 0;
    this.direction = null;
  }

  move(dx: number, dy: number): Arrow[] {
    const safeX = Number.isFinite(dx) ? dx : 0;
    const safeY = Number.isFinite(dy) ? dy : 0;
    if (safeX === 0 && safeY === 0) {
      return [];
    }

    const horizontal = Math.abs(safeX) >= Math.abs(safeY);
    const nextDirection: Arrow = horizontal
      ? safeX >= 0
        ? 'right'
        : 'left'
      : safeY >= 0
      ? 'down'
      : 'up';
    if (this.direction !== null && this.direction !== nextDirection) {
      this.x = 0;
      this.y = 0;
    }
    this.direction = nextDirection;

    if (horizontal) {
      this.x += safeX;
      const steps = Math.trunc(Math.abs(this.x) / this.step);
      if (steps === 0) {
        return [];
      }
      this.x -= Math.sign(this.x) * steps * this.step;
      return Array<Arrow>(steps).fill(safeX > 0 ? 'right' : 'left');
    }

    this.y += safeY;
    const steps = Math.trunc(Math.abs(this.y) / this.step);
    if (steps === 0) {
      return [];
    }
    this.y -= Math.sign(this.y) * steps * this.step;
    return Array<Arrow>(steps).fill(safeY > 0 ? 'down' : 'up');
  }
}

export function hintText(
  mode: InputMode,
  state: ModStates,
  os: KeyboardOS,
): string {
  if (!isModifierActive(state)) {
    if (os === 'windows') {
      return mode === 'text'
        ? keyboardStrings.windowsHints.text
        : keyboardStrings.windowsHints.keys;
    }
    return mode === 'text'
      ? keyboardStrings.hints.text
      : keyboardStrings.hints.keys;
  }

  if (os === 'windows') {
    const lockedState: ModStates = { ...NO_MODS };
    const onceState: ModStates = { ...NO_MODS };
    for (const key of Object.keys(state) as ModKey[]) {
      if (state[key] === 'lock') {
        lockedState[key] = 'lock';
      } else if (state[key] === 'once') {
        onceState[key] = 'once';
      }
    }
    const lockedPrefix = activeModifierPrefix(lockedState, os);
    const oncePrefix = activeModifierPrefix(onceState, os);
    if (lockedPrefix !== '' && oncePrefix !== '') {
      return `${lockedPrefix}${keyboardStrings.windowsHints.mixed}${oncePrefix}${keyboardStrings.windowsHints.mixedSuffix}`;
    }
    if (lockedPrefix !== '') {
      return `${lockedPrefix}${keyboardStrings.windowsHints.lockSuffix}`;
    }
    return `${oncePrefix}${keyboardStrings.windowsHints.onceSuffix}`;
  }

  const prefix = activeModifierPrefix(state, os);
  const locked = (Object.keys(state) as ModKey[]).some(
    key => state[key] === 'lock',
  );
  return `${prefix}${
    locked ? keyboardStrings.hints.lockSuffix : keyboardStrings.hints.onceSuffix
  }`;
}

function isCombiningMark(codePoint: number): boolean {
  return (
    codePoint === 0x200d ||
    (codePoint >= 0x0300 && codePoint <= 0x036f) ||
    (codePoint >= 0x0483 && codePoint <= 0x0489) ||
    (codePoint >= 0x0591 && codePoint <= 0x05bd) ||
    codePoint === 0x05bf ||
    (codePoint >= 0x05c1 && codePoint <= 0x05c2) ||
    (codePoint >= 0x05c4 && codePoint <= 0x05c5) ||
    codePoint === 0x05c7 ||
    (codePoint >= 0x0610 && codePoint <= 0x061a) ||
    (codePoint >= 0x064b && codePoint <= 0x065f) ||
    codePoint === 0x0670 ||
    (codePoint >= 0x06d6 && codePoint <= 0x06dc) ||
    (codePoint >= 0x06df && codePoint <= 0x06e4) ||
    (codePoint >= 0x06e7 && codePoint <= 0x06e8) ||
    (codePoint >= 0x06ea && codePoint <= 0x06ed) ||
    (codePoint >= 0x0711 && codePoint <= 0x0711) ||
    (codePoint >= 0x0730 && codePoint <= 0x074a) ||
    (codePoint >= 0x07a6 && codePoint <= 0x07b0) ||
    (codePoint >= 0x07eb && codePoint <= 0x07f3) ||
    (codePoint >= 0x0816 && codePoint <= 0x0819) ||
    (codePoint >= 0x081b && codePoint <= 0x0823) ||
    (codePoint >= 0x0825 && codePoint <= 0x0827) ||
    (codePoint >= 0x0829 && codePoint <= 0x082d) ||
    (codePoint >= 0x0859 && codePoint <= 0x085b) ||
    (codePoint >= 0x08d3 && codePoint <= 0x08e1) ||
    (codePoint >= 0x08e3 && codePoint <= 0x0903) ||
    (codePoint >= 0x093a && codePoint <= 0x094f) ||
    (codePoint >= 0x0951 && codePoint <= 0x0957) ||
    (codePoint >= 0x0962 && codePoint <= 0x0963) ||
    (codePoint >= 0x0981 && codePoint <= 0x0983) ||
    (codePoint >= 0x09bc && codePoint <= 0x09cd) ||
    (codePoint >= 0x0a01 && codePoint <= 0x0a03) ||
    (codePoint >= 0x0a3c && codePoint <= 0x0a51) ||
    (codePoint >= 0x0e31 && codePoint <= 0x0e31) ||
    (codePoint >= 0x0e34 && codePoint <= 0x0e3a) ||
    (codePoint >= 0x0e47 && codePoint <= 0x0e4e) ||
    (codePoint >= 0x1ab0 && codePoint <= 0x1aff) ||
    (codePoint >= 0x1dc0 && codePoint <= 0x1dff) ||
    (codePoint >= 0x20d0 && codePoint <= 0x20ff) ||
    (codePoint >= 0xfe20 && codePoint <= 0xfe2f)
  );
}

function isVariationSelector(codePoint: number): boolean {
  return (
    (codePoint >= 0xfe00 && codePoint <= 0xfe0f) ||
    (codePoint >= 0xe0100 && codePoint <= 0xe01ef)
  );
}

function isEmojiModifier(codePoint: number): boolean {
  return codePoint >= 0x1f3fb && codePoint <= 0x1f3ff;
}

function isRegionalIndicator(codePoint: number): boolean {
  return codePoint >= 0x1f1e6 && codePoint <= 0x1f1ff;
}

function isTag(codePoint: number): boolean {
  return codePoint >= 0xe0020 && codePoint <= 0xe007f;
}

function graphemes(text: string): string[] {
  const result: string[] = [];
  let cluster = '';
  let previous = 0;
  let regionalRun = 0;

  for (const character of text) {
    const codePoint = character.codePointAt(0) ?? 0;
    const combining =
      isCombiningMark(codePoint) ||
      isVariationSelector(codePoint) ||
      isEmojiModifier(codePoint) ||
      isTag(codePoint) ||
      codePoint === 0x20e3;
    const regionalPair =
      isRegionalIndicator(previous) &&
      isRegionalIndicator(codePoint) &&
      regionalRun % 2 === 1;
    const extendsCluster =
      cluster !== '' &&
      (combining ||
        previous === 0x200d ||
        (previous === 0x1f3f4 && codePoint === 0xe0067) ||
        regionalPair ||
        (previous === 0x0d && codePoint === 0x0a));

    if (extendsCluster) {
      cluster += character;
      regionalRun = isRegionalIndicator(codePoint)
        ? regionalRun + 1
        : regionalRun;
    } else {
      if (cluster !== '') {
        result.push(cluster);
      }
      cluster = character;
      regionalRun = isRegionalIndicator(codePoint) ? 1 : 0;
    }
    previous = codePoint;
  }

  if (cluster !== '') {
    result.push(cluster);
  }
  return result;
}

function utf8Length(text: string): number {
  let bytes = 0;
  for (const character of text) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint <= 0x7f) {
      bytes += 1;
    } else if (codePoint <= 0x7ff) {
      bytes += 2;
    } else if (codePoint <= 0xffff) {
      bytes += 3;
    } else {
      bytes += 4;
    }
  }
  return bytes;
}

export function splitUtf8(text: string, maxBytes = 4096): string[] {
  const limit = Math.floor(maxBytes);
  if (limit <= 0 || text === '') {
    return [];
  }

  const parts: string[] = [];
  let part = '';
  let bytes = 0;

  for (const cluster of graphemes(text)) {
    const size = utf8Length(cluster);
    if (size > limit) {
      if (part !== '') {
        parts.push(part);
        part = '';
        bytes = 0;
      }
      continue;
    }
    if (bytes + size > limit) {
      parts.push(part);
      part = '';
      bytes = 0;
    }
    part += cluster;
    bytes += size;
  }

  if (part !== '') {
    parts.push(part);
  }
  return parts;
}
