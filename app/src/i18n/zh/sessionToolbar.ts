export const sessionToolbar = {
  a11y: {
    collapse: '收起会话，连接保持',
    keyboard: {
      open: '打开键盘',
      hide: '收起键盘',
      disabled: '键盘，观看时不可用',
    },
    clipboard: '剪贴板',
    more: '更多',
    displays: '显示器和虚拟屏',
  },
  toolbarLabel: '会话工具栏',
  collapse: '收起会话，连接保持',
  keyboard: {
    open: '打开键盘',
    hide: '收起键盘',
    disabled: '键盘，观看时不可用',
  },
  modeLabel: '操作方式',
  mode: {
    trackpad: '触控板',
    touch: '触屏',
  },
  clipboard: '剪贴板',
  more: '更多',
  disconnect: '断开',
  ctrlAltDel: 'Ctrl+Alt+Del',
  ctrlAltDelA11y: '发送 Ctrl Alt Delete',
  ctrlAltDelToast: '已发送 Ctrl+Alt+Del',
  ctrlAltDelDenied: '这台电脑不允许远程发送 Ctrl+Alt+Del',
  ctrlAltDelFailed: 'Ctrl+Alt+Del 没有发送成功',
  stats: {
    rtt: (value: string) => `延迟 ${value}`,
    fps: (value: string) => `帧率 ${value}`,
    bitrate: (value: string) => `码率 ${value}`,
    decode: (value: string) => `解码 ${value}`,
    decodePair: (p50: number, p95: number) => `${p50}/${p95} ms`,
    a11y: (latency: string, fps: string, bitrate: string, decode: string) =>
      `统计浮层：延迟 ${latency}，${fps}，码率 ${bitrate}，解码 ${decode}`,
    a11yPair: (
      latency: string,
      fps: string,
      bitrate: string,
      p50: number,
      p95: number,
    ) =>
      `统计浮层：延迟 ${latency}，${fps}，码率 ${bitrate}，解码中位 ${p50} 毫秒，95 分位 ${p95} 毫秒`,
    unavailable: '—',
  },
  tips: {
    clipboard: (count: number) => `已同步剪贴板 · ${count} 字`,
    clipboardManual: (count: number) =>
      `电脑上复制了 ${count} 字，打开剪贴板面板可以取用`,
    mode: {
      touch: '触屏模式：点哪里就点哪里',
      trackpad: '触控板模式：整块屏幕都是触控板',
    },
  },
} as const;
