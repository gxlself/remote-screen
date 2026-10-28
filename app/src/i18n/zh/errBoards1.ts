/** A space after a Latin word before Chinese text. */
const space = (word: string) => (/[A-Za-z]$/.test(word) ? ' ' : '');

export const errBoards1 = {
  permission: {
    kicker: '无法开始远控',
    os: { macos: 'macOS', windows: 'Windows', unknown: '系统' },
    pathLabel: '设置路径',
    window: {
      screenTitle: '屏幕与系统录音',
      accessibilityTitle: '辅助功能',
      screenDesc: '允许下面的应用录制屏幕和系统音频内容',
      accessibilityDesc: '允许下面的应用控制你的电脑',
      remoteHost: 'Host',
      meeting: '会议软件',
      screenshot: '截图工具',
      note: (osName: string) =>
        `打开开关后，${osName}${space(osName)}会要求重启 Host，它会自动重新上线`,
    },
    screen: {
      windowTitle: '屏幕与系统录音',
      windowDesc: '允许下面的应用录制屏幕和系统音频内容',
      title: (hostName: string) => `${hostName} 还没有授予屏幕录制权限`,
      desc: (mac: boolean) =>
        mac
          ? 'Host 需要这个权限才能把画面发给你。需要有人在这台 Mac 前打开开关。'
          : 'Host 需要这个权限才能把画面发给你。需要有人在这台电脑前打开开关。',
      meta: (mac: boolean) =>
        mac ? '已在 Mac 上弹出这个设置页' : '已在电脑上弹出这个设置页',
      path: ['系统设置', '隐私与安全性', '屏幕与系统录音'],
    },
    accessibility: {
      windowTitle: '辅助功能',
      windowDesc: '允许下面的应用控制你的电脑',
      title: (hostName: string) => `${hostName} 还没有授予辅助功能权限`,
      desc: (mac: boolean) =>
        mac
          ? 'Host 需要这个权限才能把你的点击和按键送进去。需要有人在这台 Mac 前打开开关。'
          : 'Host 需要这个权限才能把你的点击和按键送进去。需要有人在这台电脑前打开开关。',
      meta: (mac: boolean) =>
        mac ? '已在 Mac 上弹出这个设置页' : '已在电脑上弹出这个设置页',
      path: ['系统设置', '隐私与安全性', '辅助功能'],
    },
    terminalDesc: '也可以先用终端连上它。',
    buttons: {
      retry: '已打开，重试',
      terminal: '改用终端',
    },
  },
  policy: {
    kicker: '没有权限',
    windowTitle: 'Headscale 策略 · 缺这一条',
    effective: '改完保存，大约 10 秒生效，不用重启',
    title: (hostName: string) => `你的账号没有访问 ${hostName} 的权限`,
    desc: 'Headscale 策略没有给你的账号放行这台电脑的远控端口。把左边这一条加进策略，换上你的账号，保存后回来重试。',
    meta: '只有能改 Headscale 策略的人才能处理',
    targetLabel: '目标',
    targetValue: 'tag:rs-host',
    portLabel: '端口',
    portValue: 'udp 7400',
    copySnippet: '复制策略片段',
    fallbackUser: '<你的账号>',
    copiedToast: '已复制策略片段',
    allowedUsers: {
      title: (hostName: string) => `${hostName} 没有允许你的账号连接`,
      desc: (mac: boolean) =>
        mac
          ? '需要在那台 Mac 的 Host 设置里加上你的账号。'
          : '需要在那台电脑的 Host 设置里加上你的账号。',
      note: 'Host 设置 › 访问 › 允许的用户',
    },
  },
  denied: {
    rejectedKicker: '对方拒绝了',
    timeoutKicker: '没有回应',
    rejectedTitle: (hostName: string) => `对方在 ${hostName} 上拒绝了这次连接`,
    timeoutTitle: (hostName: string) => `${hostName} 上 30 秒没人回应`,
    rejectedDesc: (mac: boolean) =>
      mac
        ? 'Mac 前面有人，点了「拒绝」。可以再请求一次，或者只请求观看，对方更容易同意。'
        : '电脑前面有人，点了「拒绝」。可以再请求一次，或者只请求观看，对方更容易同意。',
    timeoutDesc: '电脑前可能没人。可以再请求一次，或者改用别的方式。',
    windowTitle: (hostName: string) => `${hostName} 上的弹窗`,
    request: (deviceName: string, mac: boolean) =>
      `${deviceName} 想控制这台${mac ? ' Mac' : '电脑'}`,
    marker: '对方点了这里',
    rejectedAt: (time: string) => `${time} 拒绝`,
    timeoutAt: (time: string) => `${time} · 30 秒没人点`,
    remaining: (count: number) => `10 分钟内还能再请求 ${count} 次`,
    blocked: '请求太频繁，10 分钟后再试',
    again: '再次请求',
    viewOnly: '只请求观看',
  },
  vpn: {
    kicker: '需要公司 VPN',
    windowTitle: (hostName: string) => `到 ${hostName} 的路`,
    intranetLabel: (addr: string) => `内网 ${addr} · 需要公司 VPN`,
    intranetValue: '未连接',
    tailnetLabel: (addr: string) => `Tailscale ${addr}`,
    tailnetReady: '可以走',
    tailnetOff: '未开启',
    relay: (ms: string) => `经中继 · ${ms}`,
    lanLabel: '局域网',
    lanValue: '你不在同一个 Wi-Fi',
    note: 'App 内连接不占系统 VPN，可以和公司 VPN 同时开',
    title: (addr: string) => `${addr} 只能在公司网络里访问`,
    desc: '请先在手机上连接公司 VPN，连好后回来点重试。',
    descAndroid:
      'Android 同时只能开一个 VPN：公司 VPN 不在工作资料里的话，连上后系统 Tailscale 会断开。',
    tailnetDesc: '不想开 VPN，也可以改走 Tailscale，慢一些。',
    tailnetRelayDesc: '不想开 VPN，也可以改走 Tailscale，经中继，慢一些。',
    retry: '我已连接，重试',
    useTailnet: '改走 Tailscale',
    useTailnetSaved: '以后连这台电脑都走 Tailscale',
    undo: '撤销',
    appConnect: '改用 App 内连接',
  },
  port: {
    kicker: '端口不通',
    windowTitle: (addr: string) => `诊断 · ${addr}`,
    vpnLabel: '公司 VPN',
    routeLabel: '路由',
    routeValue: (route: string) => `${route} 经 VPN`,
    pingLabel: '主机能 ping 通',
    udpLabel: 'UDP 7400',
    tcpLabel: 'TCP 7400',
    hostIntranetLabel: 'Host 内网连接',
    timeout: '超时',
    refused: '被拒绝',
    reachable: '通',
    opened: '已打开',
    closed: '已关闭',
    reported: (relative: string) => `最后一项是 ${relative}经 Tailscale 回报的`,
    reportedLan: (relative: string) => `最后一项是 ${relative}回报的`,
    title: (hostName: string) => `连不上 ${hostName} 上的 Host`,
    desc: '公司 VPN 已连上，网络也通，但 Host 的端口连不上。',
    refusedDesc: (mac: boolean) =>
      mac
        ? 'TCP 被直接拒绝，多半是那台 Mac 没打开「允许通过内网 IP 连接」。'
        : 'TCP 被直接拒绝，多半是那台电脑没打开「允许通过内网 IP 连接」。',
    copyDiagnostics: '复制诊断信息',
    copiedToast: '已复制诊断信息',
    howTo: (mac: boolean) => (mac ? '在 Mac 上怎么打开' : '在电脑上怎么打开'),
    dialogTitle: (mac: boolean) =>
      mac ? '在那台 Mac 上打开内网连接' : '在那台电脑上打开内网连接',
    dialogBody: (mac: boolean) =>
      mac
        ? '在那台 Mac 的 Host 菜单 › 设置 › 网络 里打开「允许通过内网 IP 连接」，并让防火墙放行 UDP 和 TCP 7400。'
        : '在那台电脑的 Host 菜单 › 设置 › 网络 里打开「允许通过内网 IP 连接」，并让防火墙放行 UDP 和 TCP 7400。',
    gotIt: '知道了',
    diagnostics: {
      hostName: '主机名',
      address: '地址',
      errorCode: '错误码',
    },
  },
} as const;
