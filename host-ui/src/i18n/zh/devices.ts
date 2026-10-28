const devices = {
  title: '已配对的设备',
  listLabel: '已配对的设备列表',
  count: (count: number) => `${count} 台`,
  notConnected: '还没连过',
  lastConnected: '上次连接',
  onlyPath: (label: string) =>
    /^[A-Za-z0-9]/.test(label) ? ` · 只允许 ${label}` : ` · 只允许${label}`,
  noPath: ' · 没有可用的路',
  empty: '还没有配对的设备',
  pairDevice: '配对新设备…',
  pairHint: '在任意一条路上配对过一次，其他路都认这台设备。',
  permissionTag: {
    view_only: '仅观看',
    control: '允许控制，每次确认',
    unattended: '无人值守',
  },
  detailLabel: '设备详情',
  permission: {
    title: '权限',
    view_only: {
      title: '仅观看',
      description: '只能看画面，不能操作鼠标键盘',
    },
    control: {
      title: '允许控制（每次确认）',
      description: '每次连进来，这台 Mac 上都会弹窗，你点允许才开始',
    },
    unattended: {
      title: '无人值守',
      description: '不用确认直接连，适合你自己的设备。被控制时屏幕四周仍会亮橙色边框',
    },
  },
  paths: {
    title: '允许的连接路径',
    tailnet: {
      title: 'tailnet',
      description: '靠 tailnet 身份认人',
    },
    lan: {
      title: '局域网',
      description: '同一 Wi-Fi 时直连，靠设备密钥认人',
    },
    intranet: {
      title: '内网 IP',
      descriptionEnabled: '经公司 VPN 或内网地址连接',
      descriptionDisabled: '需要先在「网络与访问」里打开',
    },
    openNetwork: '去打开',
    keepOnePath: '至少留一条路，不然这台设备连不上',
  },
  saveFailed: '没有保存：',
  revoke: {
    action: '撤销配对',
    hint: (name: string) => `撤销后，${name} 要重新扫码或输入配对码才能连`,
    confirm: (name: string) => `确定撤销「${name}」？`,
    confirmAction: '撤销',
    cancel: '取消',
    failed: '没有撤销：',
  },
} as const;

export default devices;
