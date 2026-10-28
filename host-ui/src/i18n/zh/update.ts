const update = {
  title: '软件更新',
  heading: 'Remote Host 有新版本',
  downloaded: '已下载 · 签名已校验',
  latest: {
    heading: 'Remote Host 已是最新版本',
    currentVersion: '当前版本',
  },
  status: {
    downloading: '正在下载…',
    verifying: '正在校验签名…',
    installing: '正在安装…',
    restarting: '正在重新启动…',
    failed: '这次更新没有装上，会自动再试',
  },
  session: {
    controlling: (name: string) => `${name} 正在控制这台 Mac`,
    watching: (name: string) => `${name} 正在观看这台 Mac`,
    autoInstall: '会话结束后自动安装，不会打断你',
    manualInstall: '已取消「会话结束后安装」，需要你自己选时间',
  },
  noSessions: {
    title: '现在没有会话，可以马上安装',
    body: '安装时 Remote Host 会重新启动一下，装好后手机照常能连。',
  },
  notes: {
    title: '更新内容',
    empty: '这个版本没有写更新说明',
    fixPrefix: '修复：',
    featurePrefix: '新增：',
    fixLabel: '修复',
    featureLabel: '新增',
  },
  schedule: {
    title: '什么时候装',
    autoInstall: '会话结束后安装',
    autoInstallHint: '有人连着时不打断，断开后自动装好',
  },
  channel: {
    title: '更新频道',
    stable: '稳定版',
    beta: '测试版',
    stableHint: '测好了再推送，大多数人选这个',
    betaHint: '更早拿到新功能，偶尔会有小问题',
    changeFailed: (message: string) => `没有切换更新频道：${message}`,
  },
  disconnectAlert: (names: string) => `现在安装会断开${names}的会话。`,
  footer: {
    signed: '签名校验不过的更新包不会安装',
  },
  install: {
    now: '现在安装',
    later: '稍后',
    remindLater: '稍后提醒我',
    endAndInstall: '好，结束后安装',
    disconnectAndInstall: '断开并现在安装',
    cancel: '取消',
    failed: (message: string) => `没有开始安装：${message}`,
  },
  settings: {
    saveFailed: (message: string) => `没有保存设置：${message}`,
  },
} as const;

export default update;
