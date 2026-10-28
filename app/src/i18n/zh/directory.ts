export const directory = {
  online: '在线',
  hostNotRunning: 'Host 未运行',
  needsTailnet: '需要 Tailscale',
  /** Tailscale 关着时：电脑只有外网地址，手机连不到。 */
  otherNetwork: '不在同一个网络',
  inSession: '会话中',
  needsUpdate: '需更新',
  lastSeen: (rel: string) => `${rel}在线`,
  viaRelay: (region: string) => `经${region}中继`,
  onlineCount: (n: number) => `${n} 台在线`,
} as const;
