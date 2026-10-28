import type { ModKey, ModState, ShortcutId } from './keyBar';

export type KeyboardOS = 'macos' | 'windows';

const macModifierOrder: ModKey[] = ['cmd', 'opt', 'ctl', 'shf'];
const windowsModifierOrder: ModKey[] = ['ctl', 'opt', 'cmd', 'shf'];

const modifierNames: Record<KeyboardOS, Record<ModKey, string>> = {
  macos: {
    cmd: 'Command',
    opt: 'Option',
    ctl: 'Control',
    shf: 'Shift',
  },
  windows: {
    cmd: 'Win',
    opt: 'Alt',
    ctl: 'Ctrl',
    shf: 'Shift',
  },
};

const modifierGlyphs: Record<ModKey, string> = {
  cmd: '⌘',
  opt: '⌥',
  ctl: '⌃',
  shf: '⇧',
};

export const MAC_SHORTCUTS: ShortcutId[] = [
  'cmdC',
  'cmdV',
  'cmdZ',
  'cmdSpace',
  'cmdTab',
  'cmdW',
  'cmdQ',
];

export const WINDOWS_SHORTCUTS: ShortcutId[] = [
  'ctrlC',
  'ctrlV',
  'altTab',
  'winD',
  'ctrlAltDel',
  'ctrlShiftEsc',
  'altF4',
  'win',
  'pasteAsTyping',
];

export const shortcutLabels: Record<ShortcutId, string> = {
  cmdC: '⌘C',
  cmdV: '⌘V',
  cmdZ: '⌘Z',
  cmdSpace: '⌘␣',
  cmdTab: '⌘⇥',
  cmdW: '⌘W',
  cmdQ: '⌘Q',
  ctrlC: 'Ctrl+C',
  ctrlV: 'Ctrl+V',
  altTab: 'Alt+Tab',
  winD: 'Win+D',
  ctrlAltDel: 'Ctrl+Alt+Del',
  ctrlShiftEsc: 'Ctrl+Shift+Esc',
  altF4: 'Alt+F4',
  win: 'Win 键',
  pasteAsTyping: '粘贴为键入',
};

export const shortcutA11yLabels: Record<ShortcutId, string> = {
  cmdC: '拷贝 Command C',
  cmdV: '粘贴 Command V',
  cmdZ: '撤销 Command Z',
  cmdSpace: '聚焦搜索 Command 空格',
  cmdTab: '切换应用 Command Tab',
  cmdW: '关闭窗口 Command W',
  cmdQ: '退出应用 Command Q',
  ctrlC: '复制 Ctrl C',
  ctrlV: '粘贴 Ctrl V',
  altTab: '切换窗口 Alt Tab',
  winD: '显示桌面 Win D',
  ctrlAltDel: '安全选项 Ctrl Alt Delete',
  ctrlShiftEsc: '任务管理器 Ctrl Shift Esc',
  altF4: '关闭窗口 Alt F4',
  win: '开始菜单 Win 键',
  pasteAsTyping: '粘贴为键入：把手机剪贴板逐字敲进去',
};

export function shortcutLabel(id: ShortcutId): string {
  return shortcutLabels[id];
}

export function shortcutA11y(id: ShortcutId): string {
  return shortcutA11yLabels[id];
}

export const keyboardStrings = {
  inputModeLabel: '输入方式',
  inputMode: {
    text: '文本',
    keys: '按键',
  },
  inputModeA11y: {
    text: '文本输入',
    keys: '按键输入',
  },
  modifierState: {
    none: '未按下',
    once: '单次',
    lock: '已锁定',
  },
  hints: {
    text: '文本模式：在手机上用输入法打完整句，再发送到电脑',
    keys: '按键模式：每个键原样发送，适合快捷键和 vim',
    onceSuffix: ' 单次：作用于下一个按键后自动松开',
    lockSuffix: ' 已锁定，再点一次松开',
  },
  windowsHints: {
    text: '文本模式：用手机输入法打好整句，再发到电脑',
    keys: '按键模式：每个键原样发送，适合快捷键',
    mixed: ' 已锁定 · ',
    mixedSuffix: ' 只管下一个键',
    lockSuffix: ' 已锁定，再点一次松开',
    onceSuffix: ' 单次：作用于下一个按键后自动松开',
  },
  cmdAsCtrl: {
    glyph: '⌘',
    label: '当作 Ctrl',
  },
  keys: {
    escape: 'esc',
    tab: 'tab',
    functionKeys: 'F1–12',
    back: '返回',
    close: '收起键盘',
    home: 'home',
    end: 'end',
    pageUp: 'pg up',
    pageDown: 'pg dn',
  },
  a11y: {
    hintBar: '键盘提示',
    cmdAsCtrl: '⌘ 当作 Ctrl：外接键盘的 Command 按 Ctrl 发送',
    direction: '方向键，点一下边缘发送对应方向，按住滑动连续发送',
    arrows: {
      up: '向上',
      down: '向下',
      left: '向左',
      right: '向右',
    },
    sendFailed: '按键没发出去，请重试',
    navKeys: {
      home: 'Home 键',
      end: 'End 键',
      pageUp: '向上翻页 Page Up',
      pageDown: '向下翻页 Page Down',
    },
    functionKeys: '功能键 F1 到 F12',
    functionKey: (index: number) => `功能键 F${index}`,
    composition: (text: string) => `组合中：${text}`,
    clipboardEmpty: '剪贴板里没有文字',
    pasteAsTyping: '粘贴为键入',
    pasteConfirmTitle: '粘贴为键入',
    pasteConfirm: (count: string) => `要把 ${count} 个字逐字敲到电脑上吗？`,
    confirm: '确定',
    cancel: '取消',
    pasteReadFailed: '无法读取剪贴板',
  },
} as const;

export function modifierName(os: KeyboardOS, key: ModKey): string {
  return modifierNames[os][key];
}

export function modifierGlyph(key: ModKey): string {
  return modifierGlyphs[key];
}

export function modifierLabel(os: KeyboardOS, key: ModKey): string {
  return os === 'macos' ? modifierGlyph(key) : modifierName(os, key);
}

export function modifierKeysFor(os: KeyboardOS): ModKey[] {
  return os === 'macos' ? macModifierOrder : windowsModifierOrder;
}

export function modifierA11y(
  os: KeyboardOS,
  key: ModKey,
  state: ModState,
): string {
  return `${modifierName(os, key)}，${keyboardStrings.modifierState[state]}`;
}

export function activeModifierPrefix(
  state: Record<ModKey, ModState>,
  os: KeyboardOS,
): string {
  const order = os === 'macos' ? macModifierOrder : windowsModifierOrder;
  return order
    .filter(key => state[key] !== 'none')
    .map(key => (os === 'macos' ? modifierGlyph(key) : modifierName(os, key)))
    .join(os === 'macos' ? '' : '+');
}
