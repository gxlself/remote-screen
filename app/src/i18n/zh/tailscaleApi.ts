export const tailscaleApi = {
  row: {
    title: 'Tailscale 设备列表',
    notSet: '未设置',
    loading: '检测中…',
    loadFailed: '状态没读出来',
    configured: '已设置',
    connected: (count: number) => `已连接 · ${count} 台设备`,
    auth: '令牌无效或已过期',
    network: '连不上 Tailscale',
    networkHeadscale: '连不上控制服务器',
    http: '请求失败',
    withHost: (host: string, state: string) => `${host} · ${state}`,
  },
  sheet: {
    title: '设置 Tailscale 令牌',
    kind: {
      a11y: '设备列表来源',
      tailscale: 'Tailscale 官方',
      headscale: 'Headscale 自建',
    },
    lead: 'App 用 Tailscale 的只读令牌列出你 Tailscale 里的设备：Linux 走 SSH，Mac 走屏幕共享，Windows 走远程桌面。',
    oauth:
      '推荐：在 Tailscale 管理后台 → Settings → OAuth clients（新版叫 Trust credentials）新建一个客户端，只勾 Devices → Core → Read，复制 Client secret（tskey-client- 开头）粘贴到这里。',
    apiKey:
      '也可以用 Keys 页生成的 API access token（tskey-api- 开头），但它有完整的账号权限，最长 90 天就会过期。',
    storage: '令牌只存在这台手机的钥匙串里，App 只用它读取设备列表。',
    storageAndroid:
      '令牌用 Android 密钥库加密，只存在这台手机上，App 只用它读取设备列表。',
    headscale: {
      lead: 'App 从自建的 Headscale 读取设备列表，再看每台设备开了哪些端口：开 5900 的是 Mac，走屏幕共享；开 3389 的是 Windows，走远程桌面；开 22 的列为 SSH 服务器。',
      apiKey:
        '在 Headscale 服务器上执行 headscale apikeys create --expiration 90d，把输出的 key 粘贴到这里。过期后重新生成粘贴即可。',
      storage:
        '这个 key 有管理员权限，只存在本机钥匙串，App 只用它读取设备列表。',
      storageAndroid:
        '这个 key 有管理员权限，用 Android 密钥库加密，只存在这台手机上，App 只用它读取设备列表。',
      tokenLabel: 'Headscale API key',
      placeholder: '粘贴 API key',
    },
    controlUrlLabel: '控制服务器',
    controlUrlPlaceholder: 'https://hs.example.com',
    status: '当前状态',
    tokenLabel: 'Tailscale 令牌',
    placeholder: 'tskey-client-…',
    save: '验证并保存',
    refresh: '立即刷新',
    refreshed: (count: number) => `已刷新，找到 ${count} 台设备`,
    openAdmin: '打开 Tailscale 管理后台',
    remove: '移除令牌',
    found: (count: number) => `找到 ${count} 台设备`,
    removed: '已移除 Tailscale 令牌',
    errors: {
      auth: '令牌无效或已过期，或缺少设备读取权限',
      authHeadscale: 'API key 无效或已过期',
      network: '连不上 api.tailscale.com，检查网络',
      networkHeadscale: '连不上控制服务器，检查网络和地址',
      controlUrl: '控制服务器地址不对',
      generic: '没能读取设备列表，请稍后再试',
    },
    removeDialog: {
      title: '移除 Tailscale 令牌？',
      message: '移除后，首页不再列出 Tailscale 里的设备。',
      confirm: '移除',
    },
  },
} as const;
