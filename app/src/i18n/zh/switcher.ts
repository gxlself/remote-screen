/** 会话切换器：工具栏「切换」按钮、浮层和 ⌃⌥⌘ 快捷键。 */
export const switcher = {
  button: '切换会话',
  title: '切换会话',
  subtitle: '当前会话收起，连接保持',
  terminals: '终端',
  other: '连接其他电脑',
  otherSubtitle: '收起当前会话，去选一台电脑',
  current: '当前',
  many: (count: number) => `同时连着 ${count} 台，会更耗电`,
  kind: {
    host: '远控',
    vnc: '屏幕共享',
    rdp: '远程桌面',
    ssh: '终端',
  },
  status: {
    connected: '已连接',
    connecting: '连接中',
    reconnecting: '重连中',
    disconnected: '已断开',
  },
  /** iPad 外接键盘的快捷键，列在每一项右边。 */
  shortcut: (index: number) => `⌃⌥⌘${index}`,
  a11y: {
    /** 工具栏上的「切换」按钮。 */
    button: '切换会话',
    item: (name: string, kind: string, status: string, current: boolean) =>
      `${name}，${kind}，${status}${current ? '，当前会话' : ''}`,
    shortcut: (index: number) => `快捷键 Control Option Command ${index}`,
  },
};
