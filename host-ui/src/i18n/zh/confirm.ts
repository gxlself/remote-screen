const confirm = {
  windowTitle: '连接确认',
  title: {
    connect: (device: string) => `「${device}」想要连接这台 Mac`,
    takeover: (device: string) => `「${device}」想要接管控制`,
  },
  description: {
    pairing: '这是这台设备第一次连接。只允许你认识的人和设备。',
    takeover: (controller: string | null) =>
      controller === null
        ? '允许后它开始控制这台 Mac。'
        : `现在由 ${controller} 控制；允许后它改为观看。`,
    attended: '这台 Mac 设成了每次连接都要确认。',
    localActivity: '你刚刚在用这台 Mac，所以先问你一下。',
    intranet: '这次是从内网连进来的，需要你确认。',
    nodeMismatch: '这台设备的 tailnet 节点和配对时记录的不一样，请确认是你认识的设备。',
    fallback: '确认后才会开始。',
  },
  account: '账号',
  tailnetVerified: 'tailnet 已验证',
  device: '设备',
  network: '网络',
  fingerprint: '证书指纹',
  unknownDevice: '一台设备',
  emptyValue: '—',
  permission: {
    label: '允许的权限',
    control: '允许控制',
    controlDescription: '可以看画面、操作鼠标键盘、收发文件和剪贴板',
    viewOnly: '仅观看',
    viewOnlyDescription: '只能看画面，不能操作',
    unattended: '允许无人值守（以后不用确认）',
  },
  countdown: (seconds: number) => `${seconds} 秒后自动拒绝`,
  queued: (count: number) => `还有 ${count} 个请求在等`,
  respondFailed: (message: string) => `没有发出去：${message}`,
  allow: '允许',
  allowControl: '允许控制',
  allowViewOnly: '允许观看',
  windows: {
    unattendedHint: '以后这台手机连进来直接开始，不再弹窗。可在设置里改回来。',
    lockScreenNote: '电脑锁屏时，这个提示会出现在登录界面上',
  },
} as const;

export default confirm;
