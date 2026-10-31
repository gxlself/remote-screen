import { isCjkAuthName } from './appLock';

export const settings = {
  a11y: { cursorSlower: '光标慢一点', cursorFaster: '光标快一点' },
  title: '设置',
  account: {
    tailnetOn: (addr: string) => `本机 ${addr}`,
    tailnetOff: 'Tailscale 未连接',
    /** Tailscale 关着时：手机自己的局域网或 VPN 地址。 */
    local: (addr: string) => `本机 ${addr}`,
    avatarA11y: (name: string) => `${name} 的账号`,
  },
  security: {
    section: '安全',
    faceId: {
      title: (name: string) =>
        isCjkAuthName(name) ? `用${name}解锁` : `用 ${name} 解锁`,
      subtitle: '打开 App、进入远控前验证',
      valueOn: '已开启',
      valueOff: '关闭',
    },
    pairedHosts: {
      title: '已配对的电脑',
      count: (count: number) => `${count} 台`,
      hint: '回到首页查看电脑列表',
    },
    fingerprint: {
      title: '证书指纹校验',
      subtitle: '始终开启：首次配对时记录，指纹变化会拒绝连接',
    },
    sshKeys: {
      title: 'SSH 密钥',
      subtitle: '私钥存在安全芯片里，导不出来',
      count: (count: number) => `${count} 把`,
    },
  },
  session: {
    section: '远控默认值',
    mode: {
      title: '默认操作方式',
      trackpad: '触控板',
      touch: '触屏直点',
      trackpadHint:
        '整块屏幕都是触控板，单指移动光标、轻点单击。适合手机这种小屏。',
      touchHint: '手指点到哪里就点哪里，拖动会移动窗口。适合 iPad 大屏。',
      touchHintAndroid: '手指点到哪里就点哪里，拖动会移动窗口。适合平板大屏。',
    },
    quality: {
      title: '画质',
      sheetTitle: '默认画质',
      value: {
        auto: '自动',
        smooth: '流畅',
        clear: '清晰',
        eco: '省流量',
      },
      optionSubtitle: {
        auto: '按网络自动调',
        smooth: '优先帧率',
        clear: '优先清晰度',
        eco: '经中继或用流量时',
      },
    },
    cursorSpeed: {
      title: '光标速度',
      value: (speed: number) => `${speed.toFixed(1)}×`,
      spoken: (speed: number) => `${speed.toFixed(1)} 倍`,
      slower: '光标慢一点',
      faster: '光标快一点',
      hint: '慢：精细；快：快速划过时加速',
    },
    naturalScroll: {
      title: '自然滚动',
      subtitle: '双指往上推，页面往上走，和 iPhone 一样',
      subtitleAndroid: '双指往上推，页面往上走，和手机上滑动一样',
    },
    haptics: {
      title: '点击时轻震',
      subtitle: '单击、拖拽开始时给一下触感反馈',
    },
    sound: {
      title: '播放远端声音',
      subtitle: '会话中也可在「更多」里开关',
    },
    pasteSync: {
      title: '粘贴前先发剪贴板',
      subtitle:
        '屏幕共享、远程桌面里粘贴时，先把这台设备的剪贴板发到电脑。屏幕共享只能带英文和符号',
    },
    commandPeriod: '外接键盘没有 Esc 键时，按 ⌘. 发送 Esc',
    collapsedKeep: {
      title: '收起的会话保持',
      sheetTitle: '收起的会话保持多久',
      subtitle: '到时间断开，省电；再打开时会自己重新连接',
      value: {
        '10m': '10 分钟',
        '30m': '30 分钟',
        '2h': '2 小时',
        always: '一直保持',
      },
    },
    /** 外接键盘切换会话的快捷键，不会发给电脑。 */
    sessionShortcuts:
      '三指左右滑：切换同时连着的电脑（终端不在其中）。外接键盘：⌃⌥⌘→ / ⌃⌥⌘← 切到下一个 / 上一个会话，⌃⌥⌘1…9 切到第几个，⌃⌥⌘S 打开切换，⌃⌥⌘T 显示或隐藏工具栏。这些按键不会发给电脑',
  },
  terminal: {
    section: '终端',
    font: {
      title: '字号',
      value: (size: number) => String(size),
      sheetTitle: '终端字号',
    },
    colorScheme: {
      title: '配色',
      subtitle: '目前只有夜间配色',
    },
    tmux: {
      title: '自动接回 tmux',
      subtitle: '打开终端时进入 tmux，关掉再打开能回到原来的会话',
    },
    snippets: {
      title: '命令片段',
      count: (count: number) => `${count} 条`,
    },
  },
  diagnostics: {
    title: '诊断与日志',
    subtitle: '连接测试、导出日志',
    selfTest: '开发者自检',
  },
  footer: (appVersion: string, minHostVersion: string) =>
    `Remote Screen ${appVersion} · 要求 Host ≥ ${minHostVersion}`,
} as const;
