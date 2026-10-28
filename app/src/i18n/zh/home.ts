export const home = {
  a11y: { clearSearch: '清除搜索' },
  title: '设备',
  status: {
    tailnetOnline: (count: number) => `Tailscale · ${count} 台在线`,
    tailnetOnlineEmbedded: (count: number) =>
      `Tailscale · App 内 · ${count} 台在线`,
    tailnetDownCompanyVpn: 'Tailscale 已断开 · 公司 VPN',
    tailnetDown: 'Tailscale 已断开',
    online: (count: number) => `${count} 台在线`,
  },
  search: {
    placeholder: '搜索名称或地址',
    clear: '清除搜索',
  },
  sections: {
    computers: '电脑',
    longPress: '长按查看详情',
    servers: '服务器',
    serversRight: (count: number) => `${count} 台 · SSH`,
  },
  banners: {
    vpnConflictTitle: '公司 VPN 已连接，系统 Tailscale 被断开了',
    vpnConflictMessage:
      'iOS 同时只能开一个系统 VPN · 先断开公司 VPN，再打开 Tailscale',
    tailnetDownTitle: 'Tailscale 没有连接',
    openTailscale: '打开 Tailscale',
  },
  androidVpn: {
    message: 'Android 同时只能开一个 VPN · 先断开公司 VPN，再打开 Tailscale',
    help: '怎么办',
    sheetTitle: '为什么 Tailscale 被断开了',
    lines: [
      'Android 同一时间只能运行一个 VPN。公司 VPN 连上后，系统会断开 Tailscale。',
      '要连 Tailscale 里的电脑：先断开公司 VPN，再打开 Tailscale。',
      '公司 VPN 装在工作资料里时，两者可以同时开，这里不会提示。',
    ],
  },
  iosVpn: {
    sheetTitle: '为什么 Tailscale 被断开了',
    lines: [
      'iOS 同一时间只能开一个系统 VPN。公司 VPN 连上后，Tailscale 会被断开。',
      '要连 Tailscale 里的电脑：先在设置里断开公司 VPN，再打开 Tailscale。',
      '局域网里按地址添加的电脑不受影响，照常能连。',
    ],
  },
  embedded: {
    conflictMessageIos: 'iOS 同时只能开一个系统 VPN',
    conflictMessageAndroid: 'Android 同时只能开一个 VPN',
    useEmbedded: '改用 App 内连接',
    why: '为什么',
    loginTitle: 'App 内连接需要登录',
    loginMessage: '在浏览器里用 Headscale 账号登录，登录后自动连上',
    login: '去登录',
    switchFailed: '没能切换到 App 内连接，稍后再试',
    loginFailed: '打不开登录页，稍后再试',
    bothTitle: 'Tailscale 和公司 VPN 同时可用',
    bothMessage: 'App 内连接不占系统 VPN，两边各走各的',
    settings: '设置',
    undo: '撤销',
    sheet: {
      title: '公司 VPN 占住了系统 VPN',
      body: 'Android 同一时间只能开一个 VPN。公司 VPN 一直占着，系统 Tailscale 就起不来。',
      rows: [
        { title: '公司 VPN', detail: '', value: '已连接' },
        {
          title: '系统 Tailscale',
          detail: 'VPN 位置被占，连不上 Tailscale 里的电脑',
          value: '起不来',
        },
        {
          title: 'App 内连接',
          detail: '不占系统 VPN，只给 Remote Screen 用',
          value: '推荐',
        },
      ],
      later: '以后再说',
      workProfileTitle: '公司 VPN 装在工作资料里？',
      workProfileBody:
        '那它只管工作资料里的 App。个人资料里的 Tailscale 照常在线，两边不冲突，也不会弹这个提示。',
    },
  },
  servers: {
    tailscaleSsh: 'Tailscale SSH',
    keyAuth: '密钥登录',
    passwordAuth: '密码登录',
    viaJump: (jump: string) => `内网 · 经 ${jump}`,
    subtitle: (os: string, auth: string) =>
      os === '' ? auth : `${os} · ${auth}`,
    needsTailnet: (subtitle: string) => `${subtitle} · 需要 Tailscale`,
    /** tailnet 里还没添加的机器，点按打开「添加服务器」。 */
    suggestion: (addr: string) => `${addr} · 来自 Tailscale · 点按添加`,
    suggestionLabel: (name: string, addr: string, status: string) =>
      `${name}，${addr}，${status}，来自 Tailscale，点按添加为 SSH 服务器`,
  },
  computer: {
    noMatch: '没有匹配的电脑',
    connecting: (name: string) => `${name}，正在连接`,
  },
  empty: {
    noDevicesTitle: '还没有设备',
    noDevicesMessage: '在电脑上装好 Remote Host 后扫码配对，或按地址添加',
    addDevice: '添加设备',
    loadFailedTitle: '读不到设备列表',
    tailnetDownTitle: 'Tailscale 没有连接',
    tailnetDownMessage:
      'Tailscale 没有连接。只有在外面连回电脑时才需要它，同一网络里的电脑可以直接添加。',
  },
  live: {
    back: '回到会话',
    a11y: (hostName: string) => `回到 ${hostName} 的远控会话`,
  },
} as const;
