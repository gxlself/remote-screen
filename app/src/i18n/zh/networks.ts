export const networks = {
  screen: {
    back: '返回设置',
    backLabel: '设置',
    title: '其他网络',
    description:
      '登记公司内网、家里的网段。连着这些网络时，App 会优先走内网地址，也能认出公司 VPN 顶掉了 Tailscale。',
    /** Tailscale 关着时。 */
    descriptionLan:
      '登记公司内网、家里的网段。连着这些网络时，App 会优先走内网地址。',
  },
  sections: {
    access: '网络接入',
    other: '其他网络',
    suggestions: '从电脑上报的网段生成',
  },
  /** 外网访问总开关：关着时整块 Tailscale 设置都不出现。 */
  outside: {
    title: '外网访问（Tailscale）',
    subtitle: '人在外面要连回电脑时再开；同一 Wi-Fi 和公司内网不需要',
  },
  tailnet: {
    title: 'Tailscale',
    up: '已连接',
    down: '未连接',
    unknown: '检测中',
    modeTitle: '接入方式',
    modeA11y: '接入 Tailscale 的方式',
    switching: '正在切换',
    modes: {
      system: 'Tailscale App',
      embedded: 'App 内连接',
      auto: '自动',
    },
    switchFailed: '没切到 App 内连接，稍后再试',
    autoNeedsUrl: '自动模式要先填 Headscale 地址',
    downHint: '打开手机上的 Tailscale App 连上就好',
    systemHint:
      '用手机上已装好的 Tailscale App。注意它和公司 VPN 不能同时开，开了公司 VPN，Tailscale 就断了。',
    embeddedHint:
      'App 自己连 Tailscale，不占系统 VPN，可以和公司 VPN 同时开。只在 App 打开时在线。',
    autoHint:
      'Tailscale App 连着时用它；它断了（比如开了公司 VPN）就自动改用 App 内连接。只在 App 打开时在线。',
    controlUrlTitle: 'Headscale 地址',
    controlUrlEmpty: '未填写',
    loginTitle: '登录 App 内连接',
    loginValue: '去登录',
    loginFailed: '打不开登录页，稍后再试',
    saved: '已保存 Headscale 地址',
    cleared: '已清空 Headscale 地址',
    embeddedStates: {
      running: '已连接 · App 内',
      needsLogin: '待登录',
      starting: '连接中',
      down: '未连接',
    },
  },
  prewarm: {
    title: '提前建立连接',
    subtitle: '打开设备页时先把连接建好，点开后更快出画面',
  },
  entries: {
    add: '添加其他网络',
    addA11y: '添加其他网络',
    addByAddress: '按 IP 或主机名添加的设备',
    count: (count: number) => `${count} 台`,
    state: {
      vpn: 'VPN 已连接',
      inside: '在内网',
      none: '未连接',
    },
    subtitle: (cidrs: string[], vpnPools: string[]) => {
      const intranet = cidrs.length === 0 ? '内网' : `内网 ${cidrs.join('、')}`;
      return vpnPools.length === 0
        ? intranet
        : `${intranet} · VPN ${vpnPools.join('、')}`;
    },
    editA11y: (name: string) => `编辑 ${name}`,
    openHint: '打开其他网络列表',
    empty: '还没有登记网络',
  },
  prefs: {
    preferLocal: {
      title: '优先走内网和局域网',
      subtitle: '和 Tailscale 都通时选内网，画质上限更高',
      /** Tailscale 关着时。 */
      subtitleLan: '有多个地址能连时先走内网和局域网，画质上限更高',
    },
    warnVpnConflict: {
      title: '公司 VPN 顶掉 Tailscale 时提醒',
      subtitle: 'iPhone 同时只能开一个 VPN',
      subtitleAndroid: 'Android 同时只能开一个 VPN，工作资料里的不算',
    },
  },
  footer:
    '连着公司 VPN 时，公司里的电脑会自动改走内网地址，用起来和 Tailscale 一样。',
  /** Tailscale 关着时。 */
  footerLan: '连着公司 VPN 时，公司里的电脑会自动改走内网地址。',
  suggestions: {
    subtitle: (hostNames: string[]) => `${hostNames.join('、')} 上报`,
    add: '添加',
    addA11y: (cidr: string) => `添加 ${cidr}`,
  },
  editor: {
    saved: '已保存',
    deleted: '已删除',
    addTitle: '添加网络',
    editTitle: '编辑网络',
    name: {
      label: '名称',
      placeholder: '如 公司网络、家里',
    },
    cidrs: {
      label: '内网网段',
      placeholder: '如 10.30.0.0/16，多个用逗号隔开',
    },
    vpnPools: {
      label: 'VPN 地址池',
      placeholder: '公司 VPN 分给手机的地址段，如 10.99.0.0/16；没有就空着',
    },
    error: {
      required: '必填',
      badCidr: (bad: string[]) => `网段格式不对：${bad.join('、')}`,
      duplicateName: '已经有同名的网络了',
      save: (message: string) => `没保存上：${message}`,
      delete: (message: string) => `没删掉：${message}`,
    },
    save: '保存',
    delete: '删除网络',
    deleteA11y: (name: string) => `删除网络 ${name}`,
    deleteDialog: {
      title: (name: string) => `删除 ${name}？`,
      message: '删除后，连着这个网络时不再优先走内网，也不会提示 VPN 冲突。',
      confirm: '删除',
    },
  },
} as const;
