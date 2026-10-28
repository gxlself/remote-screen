const pair = {
  title: '配对新设备',
  qrAlt: '配对二维码',
  loading: '正在获取配对码…',
  noCode: '暂时没有配对码',
  expiry: {
    suffix: ' 后失效 · 只能用一次',
    expired: '配对码已过期',
    invalid: '已失效',
    expiredOverlay: '已过期',
  },
  instructions: {
    open: '在手机或 iPad 上打开 Remote Screen',
    addPrefix: '点右上角 ',
    addSuffix: '，选择「扫码配对」',
    scan: '对准左边的二维码，或手动输入下面的数字',
  },
  codeLabel: '配对码',
  local: '本机',
  fingerprint: '证书指纹',
  addressCount: (count: number) => ` 等 ${count} 个地址`,
  info: '只有同一个 tailnet 里的设备能完成配对。手机会记住这个指纹，之后指纹变化会拒绝连接。',
  newCode: '换一个码',
  done: '完成',
  startFailed: (message: string) => `没拿到配对码：${message}`,
  status: {
    waitingLocal: '手机已输入正确的码，请在弹出的窗口里确认',
    ok: '配对成功',
    okHint: '可以在手机上开始连接了。',
    badCode: (attemptsLeft: number) => `配对码不对，还能再试 ${attemptsLeft} 次`,
    locked: '配对码错太多次，已失效',
    denied: '你拒绝了这次配对',
    disabledOnPath: '这台 Mac 没有允许经内网配对',
    networkAccess: '网络与访问…',
  },
} as const;

export default pair;
