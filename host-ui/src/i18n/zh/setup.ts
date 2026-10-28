const setup = {
  sidebar: {
    title: 'Remote Host',
    subtitle: '首次设置 · 约 1 分钟',
    stepsLabel: '设置步骤',
    note: '每一步都可以稍后在菜单栏 › 权限与设置 里完成。',
  },
  steps: {
    tailnet: {
      title: '连接 tailnet',
      pending: '还没连上',
      done: (address: string) => `已连接 · ${address}`,
    },
    screen: {
      title: '屏幕录制',
      pending: '用于读取画面',
      waiting: '等待授权',
      done: '已授权',
    },
    ax: {
      title: '辅助功能',
      pending: '用于接收鼠标键盘',
      waiting: '等待授权',
      done: '已授权',
    },
    pair: {
      title: '配对手机',
      pending: '扫码或输入 6 位数字',
      done: (count: number) => `已配对 ${count} 台`,
    },
  },
  progress: (step: number, total: number) => `第 ${step} 步，共 ${total} 步`,
  content: {
    tailnet: {
      title: '先把这台 Mac 连上 tailnet',
      description:
        'Remote Host 只在你的 tailnet 里工作。打开 Tailscale 并登录，连上后这里会自动进入下一步。',
      waiting: '等待连接…',
    },
    screen: {
      title: '允许 Remote Host 录制屏幕',
      description:
        'Remote Host 用系统的屏幕捕获读取画面，再用硬件编码发给你的手机。画面只在你的 tailnet 里点对点加密传输，不会上传到任何服务器。',
      action: '打开系统设置',
      waiting: '等待授权…打开后这里会自动进入下一步',
      hint: 'macOS 可能会要求重新打开 Remote Host，它会自动回到这一步',
    },
    ax: {
      title: '允许 Remote Host 控制鼠标和键盘',
      description:
        '手机上的点按和键盘输入要通过辅助功能权限转给这台 Mac。只有你允许的设备才能操作。',
      action: '打开系统设置',
      waiting: '等待授权…打开后这里会自动进入下一步',
      hint: 'macOS 可能会要求重新打开 Remote Host，它会自动回到这一步',
    },
    pair: {
      title: '配对你的手机',
      description:
        '在手机或 iPad 上打开 Remote Screen，扫码或输入 6 位数字。配好之后这里会自动完成。',
      action: '显示配对二维码',
    },
  },
  visual: {
    systemSettings: '系统设置',
    privacy: '隐私与安全性',
    screenRecording: '屏幕与系统录音',
    accessibility: '辅助功能',
    appName: 'Remote Host',
    tailnet: 'tailnet',
    switchHint: '打开这个开关',
  },
  stepDone: '这一步已经完成',
  complete: {
    title: '全部设置好了',
    description: '以后在菜单栏点 Remote Host 图标就能看到谁在连接。',
  },
  windows: {
    sidebar: {
      note: '每一步以后都能在 托盘 › 设置 里改。',
    },
    steps: {
      tailnet: {
        title: '连接 tailnet',
        pending: '还没连上',
        done: (name: string | null, address: string) =>
          name === null ? address : `已登录 ${name} · ${address}`,
      },
      service: {
        title: '系统服务',
        pending: '服务没有运行',
        done: '已运行 · 锁屏时也能操作',
      },
      lan: {
        title: '内网连接',
        pending: '默认关闭，可跳过',
        unsaved: '已打开 · 待保存',
        done: (nicName: string) => (nicName.length === 0 ? '已打开' : `已打开 · ${nicName}`),
        skipped: '已跳过 · 只走 tailnet',
      },
      pair: {
        title: '配对手机',
        pending: '扫码或输入 6 位数字',
        done: (count: number) => `已配对 ${count} 台`,
      },
    },
    content: {
      tailnet: {
        title: '先把这台电脑连上 tailnet',
        description:
          'Remote Host 只在你的 tailnet 里工作。打开 Tailscale 并登录，连上后这里会自动进入下一步。',
        waiting: '等待连接…',
      },
      service: {
        title: '系统服务',
        description: 'Remote Host 通过 Windows 服务在后台运行，锁屏后也能保持连接。',
        waiting: '正在检查系统服务…',
        done: '已运行 · 锁屏时也能操作',
        stopped: '服务没有运行',
        hint: '重新安装 Remote Host 或在服务管理器里启动 RemoteHostService',
      },
      lan: {
        title: '内网连接',
        introTitle: '要不要允许从内网连进来？',
        introDescription:
          '默认只在 tailnet 地址上等手机连进来，大多数时候这就够了。用不上就直接跳过，以后随时能打开。',
        whenTitle: '什么时候需要打开',
        whenCards: [
          {
            title: '你常在家连公司 VPN，再连这台电脑',
            description: '走公司内网约 23 ms，比 tailnet 经上海中继的 48 ms 快',
          },
          {
            title: '手机就在公司内网里',
            description: '同一网段或连着公司 VPN 时，不经过 tailnet 也能连',
          },
        ],
        choicesTitle: '打开后要选这几样',
        interfacesLabel: '监听的网卡',
        sourcesLabel: '允许的来源网段',
        sourcesExample: '如公司 VPN 10.99.0.0/16',
        firewallLabel: '防火墙规则',
        firewallValue: '自动加，只对域、专用网络',
        warning:
          '打开前先确认公司 IT 允许。内网上没有 tailnet 身份，只认配对过的手机（设备密钥 + 双向证书校验）。',
        settingsHint: '以后在 设置 › 网络与访问 里也能改',
        openTitle: '内网连接已打开，选一下范围',
        openDescription:
          '手机要在公司内网里，或者先连上公司 VPN，才能走这条路。只放行下面选的网卡和网段。',
        formInterfaces: '监听的网卡',
        formSources: '允许的来源网段',
        formPort: '端口',
        formPortValue: 'UDP + TCP 7400',
        formFirewall: '防火墙',
        formFirewallValue: '保存时自动添加规则（只对域、专用网络）',
        edit: '修改',
        loadingNics: '正在读取网卡…',
        noNics: '没有可用的网卡',
        skip: '跳过，以后再说',
        open: '打开内网连接',
        save: '保存并继续',
      },
      pair: {
        title: '配对你的手机',
        description: '在手机上打开 Remote Screen，点右上角 + ，选「扫码配对」，对准下面的二维码。',
        lanHint: '在 tailnet 上或从公司 VPN 来都能配对。',
        tailnetHint: '手机要在同一个 tailnet 里才能配对。',
        passwordHint: '没人能看这块屏幕时，可以在设置里打开「用 Windows 账户密码配对」',
        action: '显示配对二维码',
        later: '稍后配对',
      },
    },
    complete: {
      description: '以后在托盘点 Remote Host 图标就能看到谁在连接。',
    },
  },
} as const;

export default setup;
