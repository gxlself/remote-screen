type EndOs = 'macos' | 'windows' | null;

/** The built-in way into a computer without Host; both when the system is unknown. */
const desktopTool = (os: EndOs) =>
  os === 'macos'
    ? '屏幕共享'
    : os === 'windows'
    ? '远程桌面'
    : '屏幕共享或远程桌面';

export const sessionEnded = {
  copyCode: (label: string) => `复制错误码 ${label}`,
  copiedCode: '已复制错误码',
  screenShareUnavailable: '没找到能打开的屏幕共享 App',
  screenShareNoAddress: (os: EndOs) =>
    `不知道这台电脑的地址，没法直接打开${desktopTool(os)}`,
  screenShareFailed: (os: EndOs) => `没能打开${desktopTool(os)}，再试一次`,
  useDesktop: (os: EndOs) => `改用${desktopTool(os)}`,
  unknownHost: '这台电脑',
  back: '返回',
  retry: '重试',
  diagnostics: '诊断',
  pairAgain: '重新配对',
  goPair: '去配对',
  a11y: {
    withSummary: (desc: string, summary: string) => `${desc} ${summary}`,
    pair: (label: string, value: string) => `${label}：${value}`,
    join: (parts: string[]) => parts.join('；'),
  },
  generic: {
    131: {
      kicker: '无法连接',
      title: (hostName: string) => `这台设备不能经内网连接 ${hostName}`,
      desc: (_hostName: string) =>
        '电脑上把这台设备的内网路径设成了「拒绝」。可以改走 Tailscale，或请电脑主人放开。',
      /** Tailscale 关着时：不提改走它。 */
      descLan: '电脑上把这台设备的内网路径设成了「拒绝」。请电脑主人放开后再试。',
    },
    132: {
      kicker: '无法连接',
      title: (hostName: string) => `${hostName} 关闭了「允许被远程控制」`,
      desc: (_hostName: string) => '需要在那台电脑的 Host 菜单里重新打开。',
    },
    142: {
      kicker: '会话已满',
      title: (hostName: string) => `${hostName} 的会话已满`,
      desc: (_hostName: string) =>
        '已经有人在控制，观看的人数也到上限了。等有人退出后再试。',
    },
    180: {
      kicker: '设备已作废',
      title: (_hostName: string) => '这台设备已被作废',
      desc: (_hostName: string) =>
        '所有电脑都不再认它。要继续使用，需要重新配对。',
    },
    181: {
      kicker: '还没配对',
      title: (hostName: string) => `这台设备还没和 ${hostName} 配对`,
      desc: (_hostName: string) => '配对一次，以后就能直接连。',
    },
    199: {
      kicker: '出了点问题',
      title: (hostName: string) => `连接 ${hostName} 时出错了`,
      desc: (_hostName: string) => '详细信息已记进日志，可以在「诊断」里导出。',
    },
    idleTimeout: {
      kicker: '连接已结束',
      title: (_hostName: string) => '空闲太久，连接已断开',
      desc: (_hostName: string) => '很久没有操作，电脑自动断开了这次连接。',
    },
    hostClosedByLocalUser: {
      kicker: '连接已结束',
      title: (_hostName: string) => '电脑前的人断开了连接',
      desc: (hostName: string) => `${hostName} 前面的人结束了这次远控。`,
    },
  },
} as const;
