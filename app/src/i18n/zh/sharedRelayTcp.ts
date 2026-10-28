export const sharedRelayTcp = {
  shared: {
    controlling: (deviceName: string) => `${deviceName} 正在控制`,
    watchingSameUser: '你在观看 · 都是你的设备，接管不用确认',
    watchingOtherUser: '你在观看 · 接管需要对方同意',
    takeOver: '接管控制',
    requestControl: '请求控制',
    handoffTitle: (deviceName: string) =>
      `已从 ${deviceName} 接管，现在由你控制`,
    handoffBody: (deviceName: string) =>
      `${deviceName} 现在只能观看，它那边也收到了提示`,
    gotIt: '知道了',
    watchers: (deviceName: string, count: number) =>
      count <= 1
        ? `${deviceName} 在观看`
        : `${deviceName} 等 ${count} 台在观看`,
    viewOnly: '仅观看：触摸和键盘不会发到电脑',
    requestTitle: (deviceName: string) => `${deviceName} 想接管控制`,
    requestBody: (timeoutS: number) =>
      `同意后你改为观看。${timeoutS} 秒内不答复就算拒绝。`,
    allow: '同意',
    deny: '拒绝',
    unsupported: '这个版本还不能答复，对方会在超时后被拒绝',
    rejected: '接管请求被拒绝',
    takingOver: '正在接管…',
    waitingFor: (deviceName: string) => `已发出请求，等 ${deviceName} 同意`,
    rejectedBecause: {
      denied: '对方拒绝了接管请求',
      timeout: '对方没有答复，接管请求已取消',
      busy: '有人正在请求控制，请稍后再试',
      view_only: '你只有观看权限，不能接管',
    } as Record<string, string>,
    noAnswer: '一直没收到答复，可以再请求一次',
    requestFailed: '请求没发出去，请重试',
  },
  path: {
    relayCaption: '后台继续尝试直连',
    intranetCaption: (name: string, addr: string) => `经${name} · ${addr}`,
    genericIntranetCaption: (addr: string) => `经内网 · ${addr}`,
    relayTitle: (
      region: string | null,
      height: number | null,
      fps: number | null,
    ) =>
      region === null
        ? '经中继转发，画质会自动降低'
        : height === null || fps === null
        ? `经${region}中继转发，画质会自动降低`
        : `经${region}中继转发，画质已自动降低`,
    relayBody: '能直接连上电脑时会自动换回来，不用手动切',
    tcpTitle: (intranet: boolean) =>
      intranet
        ? '公司网络限制了快速通道，已换成备用连接'
        : '当前网络限制了快速通道，已换成备用连接',
    tcpBody: '画面会比平时糊一点，换个网络后会自动换回来',
    gotIt: '知道了',
  },
} as const;
