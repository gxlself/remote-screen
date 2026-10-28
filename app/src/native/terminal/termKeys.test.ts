import {
  SwipeArrows,
  consumeOnce,
  cycle,
  modA11y,
  modHint,
  rowKeys,
  termMods,
} from './termKeys';

it('cycles modifiers through none, once and lock', () => {
  expect(cycle('none')).toBe('once');
  expect(cycle('once')).toBe('lock');
  expect(cycle('lock')).toBe('none');
});

it('maps modifier states to the native bit mask and consumes one-shots', () => {
  const state = { ctrl: 'once', alt: 'lock' } as const;
  expect(termMods(state)).toBe(3);
  expect(termMods(consumeOnce(state))).toBe(2);
  expect(consumeOnce(consumeOnce(state))).toEqual(consumeOnce(state));
});

it('describes active modifiers in ctrl then alt order', () => {
  expect(modHint({ ctrl: 'none', alt: 'none' })).toBe('修饰键点两下锁定');
  expect(modHint({ ctrl: 'once', alt: 'none' })).toBe('ctrl 单次');
  expect(modHint({ ctrl: 'once', alt: 'lock' })).toBe('ctrl 单次 · alt 已锁定');
});

it('describes each modifier state for screen readers', () => {
  expect(modA11y('ctrl', 'none')).toBe('ctrl，未按下');
  expect(modA11y('ctrl', 'once')).toBe('ctrl，单次，下一个键后松开');
  expect(modA11y('ctrl', 'lock')).toBe('ctrl，已锁定');
  expect(modA11y('alt', 'once')).toBe('alt，单次，下一个键后松开');
});

it('turns horizontal swipes into one arrow per 24 points', () => {
  const swipe = new SwipeArrows();
  swipe.start();
  expect(swipe.move(50, 0, 'horizontal')).toEqual(['right', 'right']);

  swipe.start();
  expect(swipe.move(0, 50, 'horizontal')).toEqual([]);

  swipe.start();
  expect(swipe.move(-48, 0, 'horizontal')).toEqual(['left', 'left']);

  swipe.start();
  expect(swipe.move(20, 0, 'horizontal')).toEqual([]);
  expect(swipe.move(-30, 0, 'horizontal')).toEqual(['left']);
});

it('uses the dominant axis for the arrow pad and resets on a turn', () => {
  const swipe = new SwipeArrows();
  swipe.start();
  expect(swipe.move(20, 0, 'any')).toEqual([]);
  expect(swipe.move(0, -30, 'any')).toEqual(['up']);

  swipe.start();
  expect(swipe.move(0, 50, 'any')).toEqual(['down', 'down']);

  swipe.start();
  expect(swipe.move(20, 0, 'any')).toEqual([]);
  expect(swipe.move(20, 0, 'any')).toEqual(['right']);
  expect(swipe.move(0, 23, 'any')).toEqual([]);
  expect(swipe.move(0, 1, 'any')).toEqual(['down']);
});

it('describes the collapsed and open key rows in canvas order', () => {
  expect(rowKeys(false)).toEqual([
    'toggleKeyboard',
    'esc',
    'ctrl',
    'alt',
    'tab',
    'arrows',
  ]);
  expect(rowKeys(true)).toEqual([
    'toggleKeyboard',
    'esc',
    'ctrl',
    'alt',
    'tab',
    'pipe',
    'tilde',
    'arrows',
  ]);
});
