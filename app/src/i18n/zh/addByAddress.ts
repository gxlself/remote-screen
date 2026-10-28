export const addByAddress = {
  title: '按地址添加',
  addressLabel: 'IP 地址或主机名',
  portLabel: 'Host 端口',
  portPlaceholder: '默认 7400',
  find: '查找',
  errors: {
    badHost: '不像是 IP 或主机名',
    badPort: '端口是 1 到 65535 的数字',
  },
  network: {
    vpn: (addr: string) => `已连接 VPN · 本机 ${addr}`,
    intranet: '在公司网络内网',
    tailnet: (addr: string) => `Tailscale 已连接 · 本机 ${addr}`,
    elapsed: (seconds: string) => `探测用了 ${seconds} 秒`,
  },
  probe: {
    running: '正在查找…',
    found: (version?: string) =>
      version === undefined ? '找到 Host' : `找到 Host ${version}`,
    notFound: '没有找到 Host',
    /** 没装 Host，但系统自带的共享或 SSH 能直接连（不装 Host 的主路径）。 */
    reachable: (via: 'vnc' | 'rdp' | 'ssh') =>
      via === 'vnc'
        ? '找到电脑 · 可以用屏幕共享'
        : via === 'rdp'
        ? '找到电脑 · 可以用远程桌面'
        : '找到电脑 · 可以用 SSH',
    noHost: '这台电脑没装 Host，不装也能直接连。',
    reasons: {
      timeout: (port: number) => `端口 ${port} 没有响应`,
      refused: (port: number) =>
        `端口 ${port} 没有开放：电脑上可能没装 Host，或者防火墙拦了`,
      noRoute: '到不了这个地址',
      needsVpn: '这个地址在公司内网，先连上公司 VPN',
      skipped: '当前版本还不能按地址查找',
      other: '连接失败',
    },
    hostSubtitle: (osVersion: string, node: string) =>
      node === '' ? osVersion : `${osVersion} · ${node}`,
  },
  known: {
    fp: (name: string) =>
      `这台电脑已经配对过，就是「${name}」，证书指纹一致。会作为它的第二个连接地址，不用重新配对。`,
    hostId: (name: string) =>
      `这台电脑已经配对过，就是「${name}」，但证书指纹和登记的不一样，建议重新配对。`,
    address: (name: string) =>
      `这个地址就是「${name}」登记的地址，会作为它的连接地址保存，不用重新配对。`,
  },
  services: {
    title: '顺便发现的其他服务',
    rdp: 'RDP 3389',
    rdpSubtitle: 'Windows 远程桌面，没装 Host 时可以兜底',
    vnc: '屏幕共享 5900',
    vncSubtitle: 'Mac 屏幕共享，没装 Host 时可以直接连',
    ssh: 'SSH 22',
    open: '开着',
    closed: '未开放',
    noResponse: '没有响应',
    openRdp: '用系统远程桌面打开',
  },
  actions: {
    intranetAddress: '内网地址',
    connectionAddress: '连接地址',
    addAddress: (network: '内网地址' | '连接地址', name: string) =>
      `添加为「${name}」的${network}`,
    saveFailed: '没保存上，稍后再试',
    addSsh: '添加为 SSH 服务器',
    pairAsNew: '当作新电脑配对',
    keepSecondAddress: '还是作为第二个地址添加',
    added: '已添加',
  },
  /** 探测全失败时也能先存下来，等电脑开机或打开共享后再连。 */
  addAnyway: {
    title: '仍然添加',
    note: '现在连不上也可以先存下来，之后在首页点它再连。',
    mac: '添加为 Mac（屏幕共享）',
    windows: '添加为 Windows（远程桌面）',
  },
  native: {
    connectVnc: '用屏幕共享连接',
    connectRdp: '用远程桌面连接',
    off: (os: 'macos' | 'windows') =>
      os === 'macos'
        ? '电脑在线，但没打开屏幕共享'
        : '电脑在线，但没打开远程桌面',
    howTo: '怎么打开',
  },
  /** 「添加电脑」页（V10H）在 native.add 之外用到的文案。 */
  lan: {
    added: '已添加',
    mac: 'Mac · 屏幕共享',
    windows: 'Windows · 远程桌面',
    cancelScan: '停止扫描',
    scanFailed: '没扫成，稍后再试',
    nameTitle: '添加这台电脑',
    nameLabel: '名字',
  },
  pair: {
    instructionsKnown: (name: string) =>
      `会多出一台，和「${name}」分开记。选一种方式：`,
    instructionsNew: '这台电脑还没配对。选一种方式：',
    code: '输入电脑上的 6 位配对码',
    codeSubtitle: '在电脑菜单栏或托盘的 Host 里点「显示配对二维码」',
    missingHello: '当前版本查不到这台电脑的 ID，请用扫码配对',
    scan: '去扫码',
  },
  a11y: {
    address: 'IP 地址或主机名',
    port: 'Host 端口',
    find: '查找',
    result: '地址探测结果',
    pairSection: '当作新电脑配对',
  },
} as const;
