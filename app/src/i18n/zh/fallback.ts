export const fallback = {
  guide: {
    rdp: {
      title: '需要 Windows App',
      message: '微软的 Windows App（原「远程桌面」）能直接打开这台电脑。',
    },
    vnc: {
      title: '需要一个屏幕共享 App',
      message:
        'iPhone 没有自带屏幕共享。装一个屏幕共享 App（比如 VNC Viewer）后再点一次；电脑上要先打开「系统设置 › 通用 › 共享 › 屏幕共享」。',
      messagePad:
        'iPad 没有自带屏幕共享。装一个屏幕共享 App（比如 VNC Viewer）后再点一次；电脑上要先打开「系统设置 › 通用 › 共享 › 屏幕共享」。',
      messageAndroid:
        '手机没有自带屏幕共享。装一个屏幕共享 App（比如 VNC Viewer）后再点一次；电脑上要先打开「系统设置 › 通用 › 共享 › 屏幕共享」。',
    },
  },
  actions: {
    store: '去 App Store',
    storeAndroid: '去 Google Play',
    cancel: '取消',
  },
} as const;
