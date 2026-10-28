const SEPS = {
  arrow: ' → ',
  chevron: ' › ',
  dot: ' · ',
  comma: '，',
  spokenTo: ' 到 ',
} as const;

function interruptedText(pct: number): string {
  return `中断于 ${pct}%`;
}

export const fileTransfers = {
  devices: {
    iphone: 'iPhone',
    ipad: 'iPad',
    phone: '手机',
  },
  directories: {
    computer: '下载',
    server: '主目录',
  },
  destinations: {
    photos: '相册',
    files: '文件 App',
  },
  separators: {
    arrow: SEPS.arrow,
    chevron: SEPS.chevron,
    dot: SEPS.dot,
    comma: SEPS.comma,
  },
  spoken: {
    to: SEPS.spokenTo,
    comma: SEPS.comma,
  },
  progress: {
    waiting: '等待中',
    verifying: '校验中…',
    paused: '已暂停',
    connecting: '正在连接…',
    value: (done: string, size: string, unit: string, pct: number) =>
      `${done} / ${size} ${unit} · ${pct}%`,
    speed: (speed: string, eta: string) => `${speed}/s · 剩余 ${eta}`,
    seconds: (n: number) => `${n} 秒`,
    minutes: (n: number) => `${n} 分钟`,
  },
  interrupted: interruptedText,
  failures: {
    io: '读写文件出错',
    sha256Mismatch: '校验没通过，要重新传',
    resumeMismatch: '续传位置对不上，要重新传',
    sourceChanged: '源文件改过了，要重新传',
    protocol: '传输出错',
    tooLarge: '文件太大',
    notFound: '远端找不到这个文件',
    permissionDenied: '没有权限读写这个位置',
    notAllowed: '这台设备没有传文件的权限',
    noSpace: '空间不够',
    authRequired: '需要先在终端里登录这台服务器',
    disabled: '对方关闭了文件传输',
    unsupported: '对方的版本不支持',
    fallback: '传输失败',
  },
  routes: {
    upload: (phone: string, source: string, directory: string) =>
      `${phone}${SEPS.arrow}${source}${SEPS.chevron}${directory}`,
    download: (source: string, directory: string) =>
      `${source}${SEPS.chevron}${directory}`,
    interruptedDownload: (source: string, phone: string, pct: number) =>
      `${source}${SEPS.arrow}${phone}${SEPS.dot}${interruptedText(pct)}`,
    interruptedUpload: (phone: string, source: string, pct: number) =>
      `${phone}${SEPS.arrow}${source}${SEPS.dot}${interruptedText(pct)}`,
    failedDownload: (source: string, phone: string, reason: string) =>
      `${source}${SEPS.arrow}${phone}${SEPS.dot}${reason}`,
    failedUpload: (phone: string, source: string, reason: string) =>
      `${phone}${SEPS.arrow}${source}${SEPS.dot}${reason}`,
  },
  completed: {
    download: (source: string, destination: string, size: string) =>
      `${source}${SEPS.arrow}${destination}${SEPS.dot}${size}`,
    upload: (phone: string, source: string, size: string) =>
      `${phone}${SEPS.arrow}${source}${SEPS.dot}${size}`,
  },
  a11y: {
    progress: (pct: number) => `已传 ${pct}%`,
    remaining: (eta: string) => `剩余 ${eta}`,
  },
} as const;
