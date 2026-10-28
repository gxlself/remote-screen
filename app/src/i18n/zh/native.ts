const tool = { macos: '屏幕共享', windows: '远程桌面' } as const;
const osShort = { macos: 'Mac', windows: 'Windows' } as const;
const osName = { macos: 'macOS', windows: 'Windows' } as const;

export const native = {
  badge: { native: '系统共享', host: 'Host' },
  home: {
    emptyTitle: '还没有电脑',
    emptyBody:
      '打开电脑自带的屏幕共享（Mac：系统设置 › 通用 › 共享）或远程桌面（Windows：设置 › 系统 › 远程桌面），就能直接连，不用装任何东西。',
    add: '添加电脑',
    otherWays: '其他方式：安装 Remote Host',
    /** 没装 Host 的电脑卡片副标题。 */
    subtitle: (os: 'macos' | 'windows') => `${osShort[os]} · ${tool[os]}`,
    /** H34：系统共享电脑还没探测出在线状态，不用看不懂的「—」。 */
    reachUnknown: '检测中…',
  },
  connectVia: {
    title: '连接方式',
    auto: '自动（先用系统共享）',
    native: (os: 'macos' | 'windows') => `只用${tool[os]}`,
    host: '只用 Host',
    /** 全局 connect.prefer 是 host 时「自动」的说法。 */
    autoHost: '自动（先用 Host）',
  },
  toast: {
    fellBack: (os: 'macos' | 'windows') =>
      `没打开${tool[os]}，已改用 Host 连接`,
  },
  add: {
    title: '添加电脑',
    lanMacs: '同一网络里的 Mac',
    lanSearching: '正在查找…',
    lanEmpty: '没找到打开了屏幕共享的 Mac',
    lanEmptyHint: '一直找不到的话，看看有没有允许远屏访问本地网络。',
    openSettings: '去设置',
    scan: '扫描局域网',
    scanning: '正在扫描…',
    scanFound: (n: number) => `找到 ${n} 台`,
    scanEmptyHint: '没扫到？试试按地址添加。',
    manual: '按地址添加',
    save: '添加并连接',
    installHost: '电脑上没法打开系统共享？安装 Host',
  },
  enable: {
    title: (os: 'macos' | 'windows') => `打开${tool[os]}`,
    lead: (os: 'macos' | 'windows', name?: string) =>
      `${name ?? (os === 'macos' ? '这台 Mac' : '这台电脑')}在线，但没打开${
        tool[os]
      }。在电脑上做一次，不用装任何东西：`,
    stepsMac: [
      '打开「系统设置 › 通用 › 共享」',
      '打开「屏幕共享」',
      '点旁边的 ⓘ，「允许访问」选「所有用户」或加入你的账户',
    ],
    stepsWin: [
      '打开「设置 › 系统 › 远程桌面」',
      '打开「远程桌面」，在弹窗里点「确认」',
      '用你的 Windows 账户密码登录（Microsoft 账户就用它的邮箱和密码）',
    ],
    retry: '已打开，重试',
    stillOff: (os: 'macos' | 'windows') =>
      `还是没打开${tool[os]}，照上面的步骤再检查一下。`,
    installHost: '还是不行？安装 Host',
    /** 适配器错误页 205 的次按钮。 */
    howTo: '怎么打开',
  },
  onboard: {
    body: '打开了屏幕共享（Mac）或远程桌面（Windows）的电脑可以直接连，不用装任何东西。',
    direct: '可以直接连接',
    directCount: (count: number) => `${count} 台`,
    subtitle: (os: string, addr: string) => `${os} · ${addr}`,
  },
  sheet: {
    os: '系统',
    osName: (os: 'macos' | 'windows') => osName[os],
    address: '地址',
    reach: '在线状态',
    /** H60：手动添加的电脑能改名字、地址和端口。 */
    rename: '名字和地址',
    editTitle: '修改这台电脑',
    nameLabel: '名字',
    hostLabel: 'IP 地址或主机名',
    portLabel: '端口',
    portPlaceholder: (port: number) => `默认 ${port}`,
    badHost: '不像是 IP 或主机名',
    badPort: '端口是 1 到 65535 的数字',
    saved: '已保存',
    save: '保存',
    remove: '删除这台电脑',
    removeTitle: (name: string) => `删除 ${name}？`,
    removeMessage: '只从电脑列表里删掉，电脑上的设置不受影响。',
    removeConfirm: '删除',
    failure: '操作失败，稍后再试',
    installHost: '安装 Host（可选）',
    notFound: '找不到这台电脑',
    /** 手动添加的电脑能换用屏幕共享或远程桌面。 */
    protocol: '连接方式',
    protocolName: { vnc: '屏幕共享 VNC', rdp: '远程桌面 RDP' },
  },
  noHost: {
    connect: (os?: 'macos' | 'windows') =>
      os === undefined ? '用屏幕共享 / 远程桌面连接' : `用${tool[os]}连接`,
    desc: (os?: 'macos' | 'windows') =>
      os === undefined
        ? '不装 Host 也能用电脑自带的屏幕共享或远程桌面直接连。'
        : `不装 Host 也能用电脑自带的${tool[os]}直接连。`,
  },
  a11y: {
    reachUnknown: '在线状态未知',
    card: (parts: string[]) => parts.join('，'),
  },
} as const;
