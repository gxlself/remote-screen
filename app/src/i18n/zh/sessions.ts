export const sessions = {
  title: '会话',
  disconnectAll: '全部断开',
  disconnectDialog: {
    title: (count: number) => `断开全部 ${count} 个会话？`,
    message: '远控和终端都会断开，tmux 里的程序不受影响。',
    confirm: '全部断开',
  },
  disconnectOne: {
    title: (name: string) => `断开 ${name}？`,
    message: '要再用时得重新连接。',
    confirm: '断开',
  },
  live: {
    section: (count: number) => `进行中 · ${count}`,
    backgroundNote: '切到后台 30 秒后暂停，1 分钟内回来都能接上',
    backgroundNoteAndroid: '切到后台也保持连接，在通知栏能回到会话或断开',
  },
  remote: {
    returnA11y: (name: string) => `回到 ${name} 的远控会话`,
    thumbHint: (status: string, display: string | null) =>
      display === null ? status : `${status}，${display}`,
    disconnectA11y: (name: string) => `断开 ${name} 的远控会话`,
    status: {
      controlling: '控制中',
      viewing: '观看中',
      connecting: '连接中',
      reconnecting: '重连中',
      disconnected: '已断开',
    },
    display: (current: number, total: number) => `显示器 ${current} / ${total}`,
    disconnect: '断开',
    returnToSession: '回到会话',
  },
  ssh: {
    connecting: '正在连接…',
    tmux: 'tmux · 断网后自动接回',
    plain: '终端',
    rowA11y: (name: string, subtitle: string) =>
      `打开 ${name} 的终端，${subtitle}`,
  },
  recent: {
    section: '最近',
    more: (count: number) => `显示全部 ${count} 条`,
    less: '收起',
    clear: '清除',
    clearDialog: {
      title: '清除最近记录？',
      message: '只清除这台手机上的记录，不影响电脑。',
      confirm: '清除',
    },
    gone: '这台电脑已不在列表里',
  },
  empty: {
    title: '还没有会话',
    message: '在「设备」里点一台电脑开始远控，或点服务器打开终端。',
    action: '去设备',
  },
  history: {
    kind: {
      remote: '远控',
      ssh: '终端',
      adapter: '屏幕共享',
    },
    adapterKind: {
      vnc: '屏幕共享',
      rdp: '远程桌面',
    },
    path: {
      direct: '直连',
      relay: (region: string) => `经${region}中继`,
      relayUnknown: '中继',
      intranet: '经内网',
      lan: '经局域网',
      manual: '手动',
    },
    subtitle: {
      remote: (duration: string, path: string | null) =>
        path === null ? `远控 · ${duration}` : `远控 · ${duration} · ${path}`,
      ssh: (duration: string) => `终端 · ${duration}`,
      adapter: (kind: string, duration: string) => `${kind} · ${duration}`,
    },
    uncleanSuffix: ' · 上次没正常结束',
    reconnectA11y: (
      name: string,
      kind: string,
      duration: string,
      when: string,
      unclean: boolean,
    ) =>
      `重新连接${name}，${kind}，${duration}，${when}${
        unclean ? '，上次没正常结束' : ''
      }`,
  },
} as const;
