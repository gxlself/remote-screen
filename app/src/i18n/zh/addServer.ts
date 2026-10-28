export const addServer = {
  title: {
    add: '手动添加服务器',
    edit: '编辑服务器',
  },
  fields: {
    name: {
      label: '名称',
      placeholder: '不填就用地址',
    },
    user: {
      label: '用户名',
    },
    host: {
      label: '地址',
      hint: 'MagicDNS 名、100.x 或内网 IP 都行',
    },
    port: {
      label: '端口',
    },
  },
  errors: {
    required: '必填',
    badHost: '不像是 IP 或主机名',
    badPort: '端口是 1 到 65535 的数字',
    duplicateName: '已经有同名的服务器了',
    tailscaleNeedsTailnet:
      '这台机器不在 Tailscale 网络里，不能用 Tailscale SSH。换成密钥或密码登录就行。',
    noKey: '这台手机还没有 SSH 密钥。先去生成一把，或者改用密码登录。',
  },
  /** 保存时第一个错误不在屏幕上，按钮旁边说清楚要改哪一项。 */
  fixAbove: (field: string) => `「${field}」还要改一下`,
  fieldNames: {
    name: '名称',
    user: '用户名',
    host: '地址',
    port: '端口',
    auth: '登录方式',
    jump: '跳板机',
  },
  auth: {
    title: '登录方式',
    options: {
      tailscale: 'Tailscale SSH',
      deviceKey: '密钥',
      password: '密码',
    },
    key: {
      use: '使用本机密钥',
      move: 'P-256 · 存在安全芯片，导不出来',
      ed25519: 'Ed25519 · 私钥加密存在这台手机上',
      copy: '复制公钥',
      empty: '还没有 SSH 密钥',
      create: '去生成',
      copyFailed: '没复制上，再试一次',
    },
    password: {
      note: '连接时在终端里输入密码。想免密登录，连上后到「SSH 密钥」把公钥装上。',
    },
    tailscale: {
      ready: '用 Tailscale 身份登录，不用密钥和密码',
      unavailable:
        '这台机器不在 Tailscale 网络里，不能用 Tailscale SSH。换成密钥或密码登录就行。',
    },
  },
  jump: {
    title: '经跳板机连接',
    subtitle: '公司内网的机器要先进跳板机',
    choose: '选择跳板机',
    current: (jump: string) => `选择跳板机，当前 ${jump}`,
    sameKey: '同一把密钥',
    sheetTitle: '选择跳板机',
    otherLabel: '其他地址',
    otherPlaceholder: 'jump-01 或 10.30.0.5:22',
    useOther: '用这个',
    otherEmpty: '先填一个地址',
    otherBad: '不像是 IP 或主机名',
    noServers: '还没有别的服务器可选，在下面填跳板机的地址。',
  },
  network: {
    title: '需要的网络',
    subtitle: '没连上时这台显示为不可用',
    none: '不需要',
    a11y: (network: string) => `需要的网络：${network}，点按更改`,
    selectedA11y: (network: string) => `${network}，已选`,
    sheetTitle: '需要的网络',
  },
  test: {
    idleTitle: '还没测试',
    idleTailscale: '先换一种登录方式，再点「测试连接」',
    idleJump: (host: string) => `${host} 在公司内网，一般要经跳板机`,
    idleHint: '点「测试连接」看看能不能连上',
    runningTitle: '正在测试…',
    okTitle: '端口能连上',
    okVia: (via: string) => `经 ${via} 端口能连上`,
    okLatency: (ms: number) => ` · ${ms} ms`,
    okSubtitle: '只测了端口。用户名和登录方式第一次连接时才验证，届时还会让你核对主机指纹',
    failTitle: '连不上',
    failReasons: {
      timeout: '3 秒内没有响应',
      refused: '端口没有开放',
      noRoute: '找不到去这个地址的路',
      needsVpn: '要先连上公司 VPN',
      noResult: '没有收到结果，稍后再试',
      unsupported: '当前版本还不能预先测试，保存后直接连接试试',
      other: '连接失败',
    },
  },
  actions: {
    test: '测试连接',
    saveAndConnect: '保存并连接',
  },
  save: {
    failed: '没保存上，稍后再试',
    added: '已添加',
    saved: '已保存',
  },
  discard: {
    title: '放弃修改？',
    message: '改动还没保存，离开后就没了。',
    confirm: '放弃',
    keep: '继续编辑',
  },
  delete: {
    row: '删除服务器',
    title: (name: string) => `删除 ${name}？`,
    message: '只删这台手机上的记录，服务器本身不受影响。',
    failed: '没删掉，稍后再试',
  },
  notFound: {
    title: '找不到这台服务器',
  },
  readOnly: '这台服务器是自动发现的，不能在这里改',
} as const;
