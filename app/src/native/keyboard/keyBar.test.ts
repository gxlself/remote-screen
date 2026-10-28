import {
  ArrowTracker,
  NO_MODS,
  consumeOnce,
  cycle,
  hintText,
  keyPress,
  mergeMods,
  modsMask,
  NAV_KEYS,
  navKeyUsage,
  pasteKeys,
  shortcut,
  splitUtf8,
} from './keyBar';

it('cycles a modifier through none, once and lock', () => {
  expect(cycle('none')).toBe('once');
  expect(cycle('once')).toBe('lock');
  expect(cycle('lock')).toBe('none');
});

it('maps modifier states to the protocol mask and consumes only one-shots', () => {
  const state = { cmd: 'once', shf: 'lock', opt: 'none', ctl: 'none' } as const;
  expect(modsMask(state)).toBe(0x0a);
  expect(modsMask(consumeOnce(state))).toBe(0x02);
  expect(consumeOnce(consumeOnce(state))).toEqual(consumeOnce(state));
});

it('presses Command around a target key and releases it afterwards', () => {
  expect(keyPress(0x06, 0x08)).toEqual([
    { usage: 0xe3, down: true, mods: 0x08 },
    { usage: 0x06, down: true, mods: 0x08 },
    { usage: 0x06, down: false, mods: 0x08 },
    { usage: 0xe3, down: false, mods: 0x00 },
  ]);
});

it('presses modifiers in Ctrl, Shift, Alt, GUI order', () => {
  expect(keyPress(0x29, 0x03)).toEqual([
    { usage: 0xe0, down: true, mods: 0x01 },
    { usage: 0xe1, down: true, mods: 0x03 },
    { usage: 0x29, down: true, mods: 0x03 },
    { usage: 0x29, down: false, mods: 0x03 },
    { usage: 0xe1, down: false, mods: 0x01 },
    { usage: 0xe0, down: false, mods: 0x00 },
  ]);
});

it('does not add modifier transitions for an unmodified press', () => {
  expect(keyPress(0x2b, 0)).toEqual([
    { usage: 0x2b, down: true, mods: 0 },
    { usage: 0x2b, down: false, mods: 0 },
  ]);
});

it('merges shortcut modifiers without double-counting sticky ones', () => {
  expect(mergeMods(0x02, 0x08)).toBe(0x0a);
  expect(mergeMods(0x08, 0x08)).toBe(0x08);
  expect(mergeMods(0x03, 0x01)).toBe(0x03);
});

it('maps shortcuts to key sequences, combos and actions', () => {
  expect(shortcut('cmdSpace', 'macos')).toEqual({
    kind: 'combo',
    combo: 'cmdSpace',
  });
  expect(shortcut('ctrlAltDel', 'windows')).toEqual({
    kind: 'action',
    action: 'ctrlAltDel',
  });
  expect(shortcut('ctrlShiftEsc', 'windows')).toEqual({
    kind: 'keys',
    events: keyPress(0x29, 0x03),
  });
  expect(shortcut('cmdC', 'macos')).toEqual({
    kind: 'keys',
    events: keyPress(0x06, 0x08),
  });
  expect(shortcut('pasteAsTyping', 'windows')).toEqual({
    kind: 'pasteAsTyping',
  });
  expect(shortcut('cmdTab', 'windows')).toEqual({
    kind: 'combo',
    combo: 'cmdTab',
  });
});

it('emits arrows once per 24 pt along the dominant axis', () => {
  const tracker = new ArrowTracker();
  tracker.start();
  expect(tracker.move(50, 0)).toEqual(['right', 'right']);

  tracker.start();
  expect(tracker.move(20, 0)).toEqual([]);
  expect(tracker.move(0, 30)).toEqual(['down']);

  tracker.start();
  expect(tracker.move(-48, 0)).toEqual(['left', 'left']);

  tracker.start();
  expect(tracker.move(20, 0)).toEqual([]);
  expect(tracker.move(-30, 0)).toEqual(['left']);
});

it('resets movement when the dominant direction changes', () => {
  const tracker = new ArrowTracker();
  tracker.start();
  expect(tracker.move(20, 0)).toEqual([]);
  expect(tracker.move(20, 0)).toEqual(['right']);
  expect(tracker.move(0, 23)).toEqual([]);
  expect(tracker.move(0, 1)).toEqual(['down']);
});

it('describes text, key and sticky modifier modes', () => {
  expect(hintText('text', NO_MODS, 'macos')).toBe(
    '文本模式：在手机上用输入法打完整句，再发送到电脑',
  );
  expect(hintText('keys', NO_MODS, 'macos')).toBe(
    '按键模式：每个键原样发送，适合快捷键和 vim',
  );
  expect(hintText('keys', { ...NO_MODS, cmd: 'once' }, 'macos')).toBe(
    '⌘ 单次：作用于下一个按键后自动松开',
  );
  expect(
    hintText('keys', { ...NO_MODS, cmd: 'lock', shf: 'lock' }, 'macos'),
  ).toBe('⌘⇧ 已锁定，再点一次松开');
});

it('describes the five Windows hints', () => {
  expect(hintText('text', NO_MODS, 'windows')).toBe(
    '文本模式：用手机输入法打好整句，再发到电脑',
  );
  expect(hintText('keys', NO_MODS, 'windows')).toBe(
    '按键模式：每个键原样发送，适合快捷键',
  );
  expect(
    hintText('keys', { ...NO_MODS, ctl: 'lock', shf: 'once' }, 'windows'),
  ).toBe('Ctrl 已锁定 · Shift 只管下一个键');
  expect(
    hintText('keys', { ...NO_MODS, ctl: 'once', opt: 'once' }, 'windows'),
  ).toBe('Ctrl+Alt 单次：作用于下一个按键后自动松开');
  expect(
    hintText('keys', { ...NO_MODS, ctl: 'lock', shf: 'lock' }, 'windows'),
  ).toBe('Ctrl+Shift 已锁定，再点一次松开');
  expect(hintText('keys', { ...NO_MODS, ctl: 'once' }, 'windows')).toBe(
    'Ctrl 单次：作用于下一个按键后自动松开',
  );
});

it('splits text on UTF-8 byte limits without splitting graphemes', () => {
  expect(splitUtf8('a'.repeat(5000))).toEqual([
    'a'.repeat(4096),
    'a'.repeat(904),
  ]);

  const family = '👨‍👩‍👧';
  const text = `${'a'.repeat(4090)}${family}e\u0301`;
  expect(splitUtf8(text)).toEqual(['a'.repeat(4090), `${family}e\u0301`]);
  expect(splitUtf8(text).join('')).toBe(text);
});

it('drops only graphemes that cannot fit in a single frame', () => {
  expect(splitUtf8('a中b', 2)).toEqual(['a', 'b']);
  expect(splitUtf8('a', 0)).toEqual([]);
  expect(splitUtf8('', 4096)).toEqual([]);
});

it('maps the navigation keys to their HID usages', () => {
  expect(NAV_KEYS.map(navKeyUsage)).toEqual([0x4a, 0x4d, 0x4b, 0x4e]);
});

it('builds a full paste and restores the Command key still held', () => {
  expect(pasteKeys('macos', 0)).toEqual([
    { usage: 0xe3, down: true, mods: 0x08 },
    { usage: 0x19, down: true, mods: 0x08 },
    { usage: 0x19, down: false, mods: 0x08 },
    { usage: 0xe3, down: false, mods: 0 },
  ]);
  expect(pasteKeys('macos', 0x88)).toEqual([
    { usage: 0xe7, down: true, mods: 0x88 },
    { usage: 0x19, down: true, mods: 0x88 },
    { usage: 0x19, down: false, mods: 0x88 },
    { usage: 0xe7, down: false, mods: 0x08 },
    { usage: 0xe7, down: true, mods: 0x88 },
  ]);
  // Shift held too: it stays down throughout.
  expect(pasteKeys('macos', 0x0a)).toEqual([
    { usage: 0xe3, down: true, mods: 0x0a },
    { usage: 0x19, down: true, mods: 0x0a },
    { usage: 0x19, down: false, mods: 0x0a },
    { usage: 0xe3, down: false, mods: 0x02 },
    { usage: 0xe3, down: true, mods: 0x0a },
  ]);
});

it('pastes with Ctrl on Windows, already mapped from Command', () => {
  expect(pasteKeys('windows', 0x01)).toEqual([
    { usage: 0xe0, down: true, mods: 0x01 },
    { usage: 0x19, down: true, mods: 0x01 },
    { usage: 0x19, down: false, mods: 0x01 },
    { usage: 0xe0, down: false, mods: 0 },
    { usage: 0xe0, down: true, mods: 0x01 },
  ]);
  expect(pasteKeys('windows', 0)).toEqual([
    { usage: 0xe0, down: true, mods: 0x01 },
    { usage: 0x19, down: true, mods: 0x01 },
    { usage: 0x19, down: false, mods: 0x01 },
    { usage: 0xe0, down: false, mods: 0 },
  ]);
});
