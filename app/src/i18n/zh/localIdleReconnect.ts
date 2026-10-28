export const localIdleReconnect = {
  localUse: {
    title: '对方正在使用这台电脑 · 你的输入先暂停',
    hint: '对方停手 1 秒后自动交还',
  },
  idle: {
    title: '长时间没有操作了',
    description: {
      macos: {
        locked: '要继续保持连接吗？倒计时结束就断开，并锁定 Mac 屏幕',
        unlocked: '要继续保持连接吗？倒计时结束就断开',
      },
      windows: {
        locked: '要继续保持连接吗？倒计时结束就断开，并锁定电脑屏幕',
        unlocked: '要继续保持连接吗？倒计时结束就断开',
      },
    },
    continueUsing: '继续使用',
  },
  reconnect: {
    hostUpdating: {
      title: '电脑正在更新',
      description: 'Remote Host 正在更新，装好后自动接回会话',
    },
    resume: {
      title: '正在恢复会话',
      description: 'App 回到前台，正在接回会话。',
    },
    reconnecting: {
      title: '正在重新连接',
      networkChanged: {
        macos: '手机的网络变了。会话还在 Mac 上保持着。',
        windows: '手机的网络变了。会话还在电脑上保持着。',
      },
      pathLost: {
        macos: '连接断了一下。会话还在 Mac 上保持着。',
        windows: '连接断了一下。会话还在电脑上保持着。',
      },
    },
    // 每一步按状态各有一句：转圈时说「正在…」，打勾后才说「已…」。
    steps: {
      tunnel: { active: '正在接通网络', done: '网络已接通' },
      host: { pending: '等电脑接回会话', active: '正在等电脑接回会话' },
      frame: { pending: '恢复画面' },
    },
    pausedAt: (time: string) => `画面已暂停在 ${time}`,
    waited: (seconds: number) =>
      `已等 ${Math.floor(
        seconds,
      )} 秒 · 60 秒内恢复不需要重新授权。重连期间的触摸和按键不会发出，避免误操作。`,
    useTerminal: '改用终端',
    collapse: '收起',
    collapseA11y: '收起，在后台继续重连',
  },
} as const;
