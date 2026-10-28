export const files = {
  title: '文件',
  sendFile: '发送文件',
  sendFileHint: '从手机选文件发到电脑或服务器',
  viewsLabel: '文件视图',
  views: {
    transfers: '传输',
    browse: '浏览电脑',
  },
  unsupported: {
    title: '文件传输还没准备好',
    message: '当前版本还不能传文件，更新后再来看看。',
  },
  messages: {
    pickFailed: (message: string) => `没发出去：${message}`,
    pickUnsupported: '当前版本还不能从手机选文件',
    importFailed: '没能接收分享来的文件',
    operationFailed: (message: string) => `操作失败：${message}`,
    sendStarted: (title: string) => `已开始发送到 ${title}`,
  },
  transfers: {
    emptyTitle: '还没有传过文件',
    emptyMessage:
      '在「浏览电脑」里轻点文件下载到手机，或者点右上角「发送文件」。',
    emptyAction: '浏览电脑',
    activeTitle: (n: number) => `进行中 · ${n}`,
    doneToday: '今天已完成',
    doneEarlier: '更早完成',
  },
  card: {
    pause: (name: string) => `暂停 ${name}`,
    resume: (name: string) => `续传 ${name}`,
    retry: (name: string) => `重试 ${name}`,
    resumeShort: '继续',
    resumeLong: '续传',
    retryShort: '重试',
    progress: (name: string) => `${name} 传输进度`,
  },
  send: {
    one: (name: string) => `发送 ${name}`,
    many: (n: number) => `发送 ${n} 个文件`,
    emptyTitle: '没有能接收文件的设备',
  },
  a11y: {
    pause: (name: string) => `暂停 ${name}`,
    target: (title: string, subtitle: string) => `${title}，${subtitle}`,
  },
} as const;

export default files;
