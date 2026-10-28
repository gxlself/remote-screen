export const filesBrowse = {
  chipsLabel: '选择电脑',
  pathLabel: '路径',
  up: '上一级',
  pathStatus: (count: number, path: string | null) =>
    path === null ? `${count} 项` : `${count} 项 · ${path}`,
  entry: {
    folder: '文件夹',
    items: (n: number) => `${n} 项`,
  },
  badges: {
    downloaded: '已在手机',
  },
  hint: '轻点文件下载到手机。',
  emptyFolder: '这个文件夹是空的',
  states: {
    noSourcesTitle: '没有能传文件的设备',
    noSourcesMessage: '配对电脑或添加服务器后，就能在这里传文件。',
    unsupportedTitle: '这台电脑还不能浏览',
    unsupportedMessage:
      '可以点右上角「发送文件」把手机上的文件发过去，会放进它的下载文件夹。',
    errorTitle: '打不开这个文件夹',
  },
  messages: {
    downloading: (name: string) => `${name} 正在下载`,
    started: (name: string) => `开始下载 ${name}，在「传输」里看进度`,
    startFailed: (message: string) => `没能开始下载：${message}`,
    shareFailed: '没能打开分享面板',
  },
  send: {
    current: (name: string) => `当前文件夹 · ${name}`,
    computer: '放进下载文件夹',
    server: '放进主目录',
  },
  a11y: {
    up: '上一级',
    folder: (name: string, subtitle: string | null) =>
      [name, filesBrowse.entry.folder, subtitle].filter(Boolean).join('，'),
    file: (name: string, subtitle: string, badge: string | null) =>
      [name, subtitle, badge].filter(Boolean).join('，'),
  },
} as const;
