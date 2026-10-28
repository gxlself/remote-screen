const qualityLabels = {
  auto: '自动',
  smooth: '流畅',
  clear: '清晰',
  eco: '省流量',
} as const;

export const sessionHud = {
  end: {
    hostShutdownToast: '电脑正在关机或重启',
    backgroundExpiredToast: '在后台太久，会话已断开',
  },
  reconnect: {
    hostUpdating: '电脑正在更新',
  },
  toolbar: {
    display: {
      label: (current: number, total: number) => `${current}/${total}`,
      a11y: (current: number, total: number) =>
        `切换显示器，当前 ${current} / ${total}`,
      entryA11y: '显示器和虚拟屏',
    },
    quality: {
      labels: qualityLabels,
      downgradedSuffix: ' · 已降低',
      a11y: (label: string) => `画质：${label}`,
      downgradedA11y: (label: string) => `画质：${label}，网络不好，已自动降低`,
      tcpA11y: (label: string) => `画质：${label}，当前网络受限，已自动降低`,
    },
  },
  hud: {
    path: {
      tailnetDirect: '直连',
      tailnetDerp: '中继',
      relayWithRegion: (region: string) => `中继 · ${region}`,
      intranet: '内网',
      intranetTcp: '内网 · TCP',
      lan: '局域网',
      manual: '手动',
      offline: '离线',
    },
    fps: (fps: number) => `${Math.round(fps)} 帧`,
    alert: '画质已自动降低',
    measuring: '连接质量：正在测量，查看详情',
    a11y: {
      direct: (latency: string, fps: string, tcp: boolean) =>
        `连接质量：直连${tcp ? '，TCP 模式' : ''} ${latency}，${fps}，查看详情`,
      relay: (
        region: string | null,
        latency: string,
        fps: string,
        tcp: boolean,
      ) =>
        `连接质量：经${region === null ? '' : region}中继${
          tcp ? '，TCP 模式' : ''
        }，${latency}，${fps}，查看详情`,
      intranet: (latency: string, fps: string, tcp: boolean) =>
        `连接质量：经公司内网${
          tcp ? '，TCP 模式' : ''
        }，${latency}，${fps}，查看详情`,
      lan: (latency: string, fps: string, tcp: boolean) =>
        `连接质量：经局域网${
          tcp ? '，TCP 模式' : ''
        }，${latency}，${fps}，查看详情`,
      manual: (latency: string, fps: string, tcp: boolean) =>
        `连接质量：手动连接${
          tcp ? '，TCP 模式' : ''
        }，${latency}，${fps}，查看详情`,
      offline: '连接质量：离线，查看详情',
    },
  },
} as const;
