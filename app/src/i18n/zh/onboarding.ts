import { isAndroidAuthName, isCjkAuthName } from './appLock';

export const onboarding = {
  step: {
    progress: (step: number, total: number) => `${step} / ${total}`,
    spoken: (step: number, total: number) => `第 ${step} 步，共 ${total} 步`,
  },
  tailnet: {
    title: '连接到你的 tailnet',
    body: '同一 Wi-Fi 里可以直接连；人在外面时，经 Tailscale 连回自己的电脑，不经过第三方服务器中转。',
    modeGroupLabel: '接入方式',
    system: {
      title: '使用手机上的 Tailscale',
      connected: '已连接',
      connectedDetail: (addr: string) =>
        `检测到 Tailscale 正在运行。本机地址 ${addr}`,
      disconnected: '未连接',
      disconnectedDetail:
        '没有检测到 Tailscale。它只用来在外面连回电脑；只在同一网络里用的话，直接点「继续」。',
      detecting: '正在检测…',
      detectingDetail: '正在检查这台手机的 Tailscale 状态。',
    },
    embedded: {
      title: '在 App 内连接',
      beta: '实验性',
      detail:
        '填写 Headscale 地址并在浏览器登录。不占用系统 VPN，可以和公司 VPN 同时开着。',
    },
    openTailscale: '打开 Tailscale',
    continue: '继续',
    /** 「在 App 内连接」切换没成功时，留在本页说明原因。 */
    embeddedUnavailable: '这台手机现在还不能在 App 内连接，先用手机上的 Tailscale。',
    switchFailed: '没切换成功，稍后再试',
    settingsHint: '之后可以在 设置 › 网络接入 中更改',
  },
  protect: {
    title: '保护这个 App',
    body: '手机落在别人手里，也打不开它去控制你的电脑。这几项之后都能在设置里改。',
    faceId: {
      title: (name: string) =>
        isCjkAuthName(name) ? `${name}解锁` : `${name} 解锁`,
      subtitle: '打开 App、进入远控前先验证',
      authReason: (name: string) =>
        isCjkAuthName(name) ? `开启${name}解锁` : `开启 ${name} 解锁`,
      unavailable: (name: string) =>
        isAndroidAuthName(name)
          ? '这台设备没有设置生物识别或密码'
          : `这台设备没有设置 ${name} 或密码`,
    },
    notify: {
      title: '通知',
      subtitle: '会话被对方断开、文件传完时提醒',
    },
    localNetwork: {
      title: '本地网络权限',
      eyebrow: '局域网',
      detail:
        '用来发现同一 Wi-Fi 里的电脑，局域网直连更快。不开也能用，只是都走 Tailscale。',
      /** Tailscale 关着时：不提它。 */
      detailLan:
        '用来发现同一 Wi-Fi 里的电脑，局域网直连更快。不开的话，只能按地址手动添加电脑。',
      allow: '允许',
      granted: '已允许',
      hintBefore: '点「允许」后，iOS 会再问你一次。',
      hintAfter: '已允许，会自动发现同一网络里的电脑',
    },
    back: '上一步',
    start: '开始使用',
    settingsHint: '之后可以在 设置 › 安全 中更改',
  },
} as const;
