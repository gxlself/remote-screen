export const deviceRoutes = {
  back: '返回设备详情',
  a11y: {
    back: '返回设备详情',
    addressHeader: (summary: string) => `地址，${summary}`,
    retest: '重新测试',
  },
  addressSection: '地址',
  addAddress: '添加地址',
  explanation:
    '每次连接会同时试所有地址，谁先通用谁；连上后每 10 秒比一次，有更快的路会无缝切过去。',
  intranetFallback: '公司内网',
  vpnConnected: '需要公司 VPN · 已连接',
  vpnDisconnected: '需要公司 VPN · 未连接',
  tailnetTitle: 'tailnet',
  tailnetEmbeddedTitle: 'tailnet · App 内',
  peerOffline: (node: string) => `${node} · 对方不在线`,
  lanTitle: '局域网',
  sameNetwork: '同一网络',
  differentNetwork: '不在同一网络 · 人在工位时才用得上',
  viaDirect: (node: string) => `${node} · 直连`,
  viaRelay: (node: string, region: string) => `${node} · 经${region}中继`,
  summaryTesting: (count: number) => `${count} 个 · 测试中…`,
  summaryTested: (count: number) => `${count} 个 · 刚刚测过`,
  current: '当前在用',
  policySection: '选择策略',
  policies: {
    auto: {
      title: '自动选最快（推荐）',
      fastest: (kind: string, ms: number) =>
        `现在是${kind} ${Math.round(ms)} ms 最快`,
      untested: '测完后显示哪条最快',
      kind: {
        intranet: '公司内网',
        lan: '局域网',
        tailnet: 'tailnet',
      },
    },
    tailnet: {
      title: '优先 tailnet',
      subtitle: 'tailnet 通就走它，内网只在断了时顶上',
    },
    intranet: {
      title: '只用内网',
      subtitle: '不走 tailnet，没连 VPN 时就连不上',
      /** Tailscale 关着时。 */
      subtitleLan: '只连内网地址，没连 VPN 时就连不上',
    },
  },
  switches: {
    tcpFallback: {
      title: '公司网络拦 UDP 时改走 TCP',
      subtitle: '画质会降一点，会话里标「TCP 模式」',
    },
    autoUpdateAddr: {
      title: '内网地址变了自动更新',
      subtitle: '电脑经 tailnet 告诉手机新地址',
      /** Tailscale 关着时。 */
      subtitleLan: '电脑换了地址会告诉手机',
    },
  },
  probeFailed: '测试失败，稍后再试',
} as const;
