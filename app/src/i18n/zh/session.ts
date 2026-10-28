export const session = {
  authenticate: (hostName: string) => `验证身份后连接 ${hostName}`,
  videoLabel: {
    trackpad: (hostName: string) =>
      `${hostName} 远端画面，双指双击切换直接触控`,
    touch: (hostName: string) => `${hostName} 远端画面，双指双击切换触控板`,
  },
  videoLabelAndroid: {
    trackpad: (hostName: string) => `${hostName} 远端画面，触控板模式`,
    touch: (hostName: string) => `${hostName} 远端画面，直接触控模式`,
  },
  a11y: {
    connected: (hostName: string) => `已连接 ${hostName}`,
  },
  hostShutdown: '电脑正在关机或重启',
  disconnect: {
    title: (hostName: string) => `断开 ${hostName}？`,
    body: '断开后要重新连接才能继续用。',
    bodyRecording: '正在录制，断开后录制会停止，录到的部分会保存在本机。',
    confirm: '断开',
    cancel: '取消',
  },
  toolbar: {
    hiddenHint: '工具栏已隐藏，三指轻点或点底部的把手可恢复',
    handle: '显示工具栏',
  },
  quality: {
    changed: (label: string) => `画质已改为「${label}」`,
    failed: (label: string) => `画质没改成，还是「${label}」`,
    retry: '重试',
  },
  display: {
    switched: (number: number) => `已切到显示器 ${number}`,
  },
} as const;
