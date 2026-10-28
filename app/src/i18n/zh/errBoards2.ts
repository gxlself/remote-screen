function joinHost(hostName: string, suffix: string): string {
  return /[A-Za-z0-9]$/.test(hostName)
    ? `${hostName} ${suffix}`
    : `${hostName}${suffix}`;
}

export const errBoards2 = {
  offline: {
    kicker: '设备离线',
    title: (hostName: string) => joinHost(hostName, '离线了'),
    descLastSeen: (relative: string) => `最后在线是 ${relative}。`,
    descRetry: '等它开机联网后再重试。',
    cardTitle: (hostName: string) => `${hostName} · 离线`,
    rowTitle: (hostName: string) => hostName,
    rowSubtitle: (osName: string | null, relative: string) =>
      osName === null ? `${relative}在线` : `${osName} · ${relative}在线`,
    lastSeenLabel: '最后在线',
    viewRelays: '查看同网段设备',
    relayDialogTitle: '同网段里在线的设备',
    connectRelay: (relayName: string) => `连接 ${relayName}`,
    unknown: '—',
  },
  noHost: {
    kicker: '没有运行 Host',
    title: (hostName: string) => joinHost(hostName, '在线，但没有运行 Host'),
    desc: '可能是重装系统后还没装回来，或者被退出了。',
    retry: '已装好，重试',
  },
  fingerprint: {
    kicker: '身份校验失败 · 已停止连接',
    title: (hostName: string) => joinHost(hostName, '的身份和上次配对时不一致'),
    desc: '可能是重装了 Host，也可能有人在冒充这台电脑。还没有发送任何画面和输入。确认是你自己重装的，再重新配对。',
    meta: '已记入连接记录',
    windowTitle: (hostName: string) => `证书指纹 · ${hostName}`,
    previousLabel: (date: string) => `上次配对 · ${date}`,
    currentLabel: '这次连接 · 刚刚',
    fingerprintPrefix: 'SHA256 ',
    unknownDate: '—',
    defaultPath: 'Tailscale',
    versionArrow: (before: string, after: string) => `${before} → ${after}`,
    addressLabel: '地址',
    hostVersionLabel: 'Host 版本',
    unknownVersion: '未知',
    note: (os: 'macos' | 'windows' | null) =>
      os === 'macos'
        ? '没重装过就别继续：到那台 Mac 上看看菜单栏里的 Host'
        : os === 'windows'
        ? '没重装过就别继续：到那台电脑上看看任务栏里的 Host'
        : '没重装过就别继续：到那台电脑上看看 Host 还在不在',
    cancel: '取消',
    rePair: '我重装过，重新配对',
    confirmTitle: '确定是你重装的吗？',
    confirmBody: (hostName: string) =>
      `会删掉 ${joinHost(
        hostName,
        '的旧指纹，然后重新配对。如果不是你重装的，可能有人在冒充它。',
      )}`,
    forgetAndPair: '删掉旧指纹并配对',
    forgetFailed: (message: string) => `没删掉旧指纹：${message}`,
  },
} as const;
