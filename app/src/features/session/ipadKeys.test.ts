import { ipadKeysFor } from './ipadKeys';

it('lists the intercepted Mac shortcuts, leaving the globe unsupported', () => {
  const keys = ipadKeysFor('macos');

  expect(keys.map(key => key.combo)).toEqual([
    'cmdTab',
    'cmdSpace',
    'ctrlSpace',
    'cmdH',
    'cmdShift4',
    null,
  ]);
  expect(keys.map(key => key.label)).toEqual([
    '⌘⇥',
    '⌘ 空格',
    '⌃ 空格',
    '⌘H',
    '⇧⌘4',
    '地球键',
  ]);
  expect(keys[5]).toMatchObject({
    id: 'globe',
    label: '地球键',
    a11y: '发送地球键给 Mac',
    combo: null,
  });
});

it('names what each new Mac shortcut does', () => {
  const byId = new Map(ipadKeysFor('macos').map(key => [key.id, key.a11y]));

  expect(byId.get('ctrlSpace')).toBe('发送 Control-空格给 Mac，切换输入法');
  expect(byId.get('cmdH')).toBe('发送 Command-H 给 Mac，隐藏当前应用');
  expect(byId.get('cmdShift4')).toBe(
    '发送 Shift-Command-4 给 Mac，截取所选区域',
  );
});

it('lists Alt-Tab, the Windows key and Win-Space for Windows hosts', () => {
  const keys = ipadKeysFor('windows');

  expect(keys.map(key => key.combo)).toEqual(['altTab', 'winKey', 'winSpace']);
  expect(keys.map(key => key.a11y)).toEqual([
    '发送 Alt-Tab 给电脑',
    '发送 Windows 键给电脑',
    '发送 Win-空格给电脑，切换输入法',
  ]);
});
