import { isCjkAuthName } from './appLock';

export const pairWaiting = {
  a11y: {
    cancel: '取消配对',
    status: (title: string, detail: string) => `${title}，${detail}`,
  },
  header: {
    title: '配对电脑',
    cancel: '取消配对',
  },
  title: (hostName: string) => `正在和 ${hostName} 配对`,
  subtitle: (
    node: string,
    address: string | null,
    hostVersion: string | null,
  ) =>
    [node, address, hostVersion === null ? null : `Host ${hostVersion}`]
      .filter((part): part is string => part !== null && part.length > 0)
      .join(' · '),
  progress: '配对进度',
  fingerprint: (display: string) => `指纹 ${display.split(' ').join('，')}`,
  verifyMatched: (host: string) =>
    host === '电脑' ? '和电脑屏幕上的一致' : `和 ${host} 屏幕上的一致`,
  approveTitle: (host: string) =>
    host === '电脑'
      ? '等待你在电脑上点「允许」'
      : `等待你在 ${host} 上点「允许」`,
  approveDetail: (host: string) =>
    host === '电脑'
      ? '确认窗口已经弹在电脑屏幕正中间'
      : `确认窗口已经弹在 ${host} 屏幕正中间`,
  permissionTitle: (host: string) =>
    host === '电脑' ? '电脑上会问你给多大权限' : `${host} 上会问你给多大权限`,
  timeoutHint: (host: string, seconds: number, method: 'qr' | 'code') =>
    `${seconds} 秒内没人点，${host}${host === '电脑' ? '' : ' '}会自动拒绝，${
      method === 'qr' ? '重新扫码就行' : '重新输入配对码就行'
    }`,
  unknownHost: '这台电脑',
  invalidQr: '二维码无效',
  byAddress: {
    probing: '正在连接这台电脑…',
    notHost: '这个地址上没有 Host。',
  },
  pairDone: {
    hostWords: { computer: '电脑', mac: 'Mac' },
    title: (hostName: string) => `${hostName} 已配对`,
    permissionTitle: (host: string) =>
      host === '电脑' ? '电脑给了这台手机' : `${host} 给了这台手机`,
    permission: {
      allowControl: '允许控制',
      unattended: '无人值守',
      viewOnly: '仅观看',
    },
    permissionNote: {
      unattended: (host: string, os: 'macos' | 'windows' | null) =>
        `以后连这台不用再去${
          host === '电脑' ? host : ` ${host} `
        }前点允许。想收回，在${
          os === 'macos' ? '菜单栏' : os === 'windows' ? '托盘' : '菜单栏或托盘'
        }的 Host 里关掉就行。`,
      other: (host: string) =>
        `每次连接都会在${host === '电脑' ? host : ` ${host} `}上请你确认`,
    },
    addresses: {
      title: '学到的连接地址',
      count: (n: number) => `${n} 个`,
      tailnetSubtitle: '在哪都能连',
      /** Tailscale 关着时，tailnet 地址只说是电脑的另一个地址。 */
      otherSubtitle: '电脑的另一个地址',
      currentSuffix: ' · 刚才就是走这条',
      lanSubtitle: '在家时自动走局域网，更快',
      sameWifi: '同一 Wi-Fi',
      intranetTitle: (address: string) => `公司内网 ${address}`,
      intranetSubtitle: '连着公司 VPN 时走这条',
      note: '两条路都认这台手机的设备密钥，换网络不用重新配对。',
    },
    sensitive: {
      title: (name: string) =>
        isCjkAuthName(name) ? `连接前验证${name}` : `连接前验证 ${name}`,
      subtitle: '只对这台电脑生效',
    },
    connectNow: '现在连接',
    backToDevices: '回到设备',
  },
} as const;
