export const ipadSession = {
  shortcutsLabel: {
    mac: 'iPad 会拦截的快捷键，点一下发给 Mac',
    windows: 'iPad 会拦截的快捷键，点一下发给电脑',
  },
  keys: {
    cmdTab: {
      label: '⌘⇥',
      a11y: '发送 Command-Tab 给 Mac',
    },
    cmdSpace: {
      label: '⌘ 空格',
      a11y: '发送 Command-空格 给 Mac',
    },
    ctrlSpace: {
      label: '⌃ 空格',
      a11y: '发送 Control-空格给 Mac，切换输入法',
    },
    cmdH: {
      label: '⌘H',
      a11y: '发送 Command-H 给 Mac，隐藏当前应用',
    },
    cmdShift4: {
      label: '⇧⌘4',
      a11y: '发送 Shift-Command-4 给 Mac，截取所选区域',
    },
    globe: {
      label: '地球键',
      a11y: '发送地球键给 Mac',
    },
    altTab: {
      label: 'Alt+Tab',
      a11y: '发送 Alt-Tab 给电脑',
    },
    winKey: {
      label: 'Win',
      a11y: '发送 Windows 键给电脑',
    },
    winSpace: {
      label: 'Win+空格',
      a11y: '发送 Win-空格给电脑，切换输入法',
    },
  },
  pointer: {
    split: '分屏时不锁定指针 · 全屏后可锁定',
    splitA11y: '分屏时不锁定指针，全屏后可锁定',
    escaped: '指针已释放 · 点画面重新锁定',
    escapedA11y: '指针已释放，点画面重新锁定',
  },
  unsupportedSuffix: '，暂不支持',
  viewerSuffix: '，观看时不可用',
} as const;
