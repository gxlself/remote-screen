export const qualityDisplay = {
  screen: {
    title: '连接与画质',
  },
  quality: {
    title: '画质',
    a11y: '画质',
    labels: {
      auto: '自动',
      smooth: '流畅',
      clear: '清晰',
      eco: '省流量',
    },
    desc: {
      auto: '按网络自动调整。网络好时又清楚又顺滑，变差时先降清晰度，再降流畅度。',
      smooth: '优先流畅：画面稍软一点，网络差或经中继时也不容易卡。',
      clear: '优先清晰度：文字和代码更锐利，网络紧张时可能没那么顺滑。',
      eco: '省流量：清晰度和流畅度都降低，适合用手机流量时。',
    },
    subtitle: (hostName: string, displayName: string | null) =>
      displayName === null ? hostName : `${hostName} · ${displayName}`,
  },
  stats: {
    title: '实时统计',
    empty: '—',
    labels: {
      latency: '延迟',
      fps: '帧率',
      bitrate: '码率',
      resolution: '分辨率',
      loss: '丢包',
      codec: '编码',
    },
    codec: {
      hevc: 'HEVC',
      h264: 'H.264',
    },
    a11y: (
      latency: string,
      fps: string,
      bitrate: string,
      resolution: string,
      loss: string,
      codec: string,
    ) =>
      `延迟 ${latency}，帧率 ${fps}，码率 ${bitrate}，分辨率 ${resolution}，丢包 ${loss}，编码 ${codec}`,
  },
  hud: {
    label: '统计浮层',
  },
  actions: {
    disconnect: '断开',
  },
  a11y: {
    close: '关闭面板，回到画面',
  },
  display: {
    title: '选择要看的显示器',
    subtitle: (hostName: string) =>
      `${hostName} · 按 Mac 上「显示器排列」的位置摆放`,
    subtitleWindows: (hostName: string) =>
      `${hostName} · 按电脑上「显示设置」里的位置摆放`,
    switchHint: '点一下切到这块显示器',
    group: '显示器',
    viewing: '正在看',
    name: (number: number) => `显示器 ${number}`,
    primarySuffix: ' · 主显示器',
    portraitSuffix: ' · 竖放',
    meta: (resolution: string, refreshHz: number | null) =>
      refreshHz === null ? resolution : `${resolution} · ${refreshHz} Hz`,
  },
} as const;
