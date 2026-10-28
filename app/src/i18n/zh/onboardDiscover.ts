export const onboardDiscover = {
  title: (total: number) => `找到了 ${total} 台电脑`,
  findingTitle: '正在查找电脑',
  body: '同一 Wi-Fi 或公司内网里的电脑都在这里。先连最常用的那台，其他的随时能加。',
  loadingDirectory: '正在读取电脑列表…',
  directoryFailed: {
    title: '读取电脑列表失败',
    message: '检查网络连接，然后重试',
  },
  groups: {
    remote: '可以远控',
    remoteCount: (count: number) => `已装 Host · ${count} 台`,
    ssh: '可以 SSH',
    sshCount: (count: number) => `不用装东西 · ${count} 台`,
  },
  computer: {
    pair: '配对',
    paired: '已配对',
    pairA11y: (name: string) => `配对 ${name}`,
    osFallback: {
      macos: 'macOS',
      windows: 'Windows',
    },
    subtitle: (os: string, path: string, latency: string) =>
      `${os} · ${path} ${latency}`,
  },
  ssh: {
    readySubtitle: 'Tailscale SSH 已开 · 不用密钥',
    keySubtitle: (os: string) => `${os} · 需要密钥`,
    direct: '直接能用',
    configureA11y: (name: string) => `设置 ${name}`,
  },
  addByAddress: {
    title: '按地址添加',
    subtitle: '不在 Tailscale 里的电脑，比如公司 VPN 里的工位',
    /** Tailscale 关着时。 */
    subtitleLan: '同一 Wi-Fi 或公司内网里的电脑，填 IP 或名字就行',
  },
  empty: '还没有找到装了 Host 的电脑，可以先继续，装好后会出现在首页',
  deviceName: {
    withUser: (user: string, model: string) => `${user} 的 ${model}`,
    mine: (model: string) => `我的 ${model}`,
  },
  a11y: {
    loadingDirectory: '正在读取电脑列表',
  },
} as const;
