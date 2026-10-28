import { terminalBarStrings } from './strings';

export type ModState = 'none' | 'once' | 'lock';

export type TermMods = {
  ctrl: ModState;
  alt: ModState;
};

export type SwipeDirection = 'left' | 'right' | 'up' | 'down';

export type SwipeMode = 'horizontal' | 'any';

export type RowKey =
  | 'toggleKeyboard'
  | 'esc'
  | 'ctrl'
  | 'alt'
  | 'tab'
  | 'pipe'
  | 'tilde'
  | 'arrows';

export const SWIPE_STEP_PT = 24;

const MOD_ORDER = ['ctrl', 'alt'] as const;

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

export function termMods(state: TermMods): number {
  return (state.ctrl === 'none' ? 0 : 1) + (state.alt === 'none' ? 0 : 2);
}

export function consumeOnce(state: TermMods): TermMods {
  return {
    ctrl: state.ctrl === 'once' ? 'none' : state.ctrl,
    alt: state.alt === 'once' ? 'none' : state.alt,
  };
}

export function modHint(state: TermMods): string {
  const active = MOD_ORDER.filter(key => state[key] !== 'none');
  if (active.length === 0) {
    return terminalBarStrings.hintNoMods;
  }
  return active
    .map(
      key =>
        `${terminalBarStrings.modifierName[key]} ${
          terminalBarStrings.modifierShortState[state[key]]
        }`,
    )
    .join(' · ');
}

export function modA11y(name: 'ctrl' | 'alt', state: ModState): string {
  return `${terminalBarStrings.modifierName[name]}，${terminalBarStrings.modifierA11yState[state]}`;
}

export class SwipeArrows {
  private x = 0;
  private y = 0;
  private direction: SwipeDirection | null = null;

  constructor(private readonly step = SWIPE_STEP_PT) {}

  start(): void {
    this.x = 0;
    this.y = 0;
    this.direction = null;
  }

  move(dx: number, dy: number, mode: SwipeMode): SwipeDirection[] {
    const safeX = Number.isFinite(dx) ? dx : 0;
    const safeY = Number.isFinite(dy) ? dy : 0;
    if (mode === 'horizontal') {
      return this.moveHorizontal(safeX);
    }
    return this.moveAny(safeX, safeY);
  }

  private moveHorizontal(dx: number): SwipeDirection[] {
    if (dx === 0) {
      return [];
    }
    const nextDirection: SwipeDirection = dx > 0 ? 'right' : 'left';
    if (this.direction !== null && this.direction !== nextDirection) {
      this.x = 0;
    }
    this.direction = nextDirection;
    this.x += dx;
    const steps = Math.trunc(Math.abs(this.x) / this.step);
    if (steps === 0) {
      return [];
    }
    this.x -= Math.sign(this.x) * steps * this.step;
    return Array<SwipeDirection>(steps).fill(nextDirection);
  }

  private moveAny(dx: number, dy: number): SwipeDirection[] {
    if (dx === 0 && dy === 0) {
      return [];
    }
    const horizontal = Math.abs(dx) >= Math.abs(dy);
    const nextDirection: SwipeDirection = horizontal
      ? dx > 0
        ? 'right'
        : 'left'
      : dy > 0
      ? 'down'
      : 'up';
    if (this.direction !== null && this.direction !== nextDirection) {
      this.x = 0;
      this.y = 0;
    }
    this.direction = nextDirection;

    if (horizontal) {
      this.x += dx;
      const steps = Math.trunc(Math.abs(this.x) / this.step);
      if (steps === 0) {
        return [];
      }
      this.x -= Math.sign(this.x) * steps * this.step;
      return Array<SwipeDirection>(steps).fill(nextDirection);
    }

    this.y += dy;
    const steps = Math.trunc(Math.abs(this.y) / this.step);
    if (steps === 0) {
      return [];
    }
    this.y -= Math.sign(this.y) * steps * this.step;
    return Array<SwipeDirection>(steps).fill(nextDirection);
  }
}

const COLLAPSED_ROW: RowKey[] = [
  'toggleKeyboard',
  'esc',
  'ctrl',
  'alt',
  'tab',
  'arrows',
];
const KEYBOARD_ROW: RowKey[] = [
  'toggleKeyboard',
  'esc',
  'ctrl',
  'alt',
  'tab',
  'pipe',
  'tilde',
  'arrows',
];

export function rowKeys(keyboardOpen: boolean): RowKey[] {
  return keyboardOpen ? [...KEYBOARD_ROW] : [...COLLAPSED_ROW];
}
