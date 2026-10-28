type EndOs = 'macos' | 'windows' | null;

export const errBoards3 = {
  version: {
    kicker: {
      old: '版本太旧',
      appOld: 'App 版本太旧',
    },
    title: {
      old: (hostName: string) => `${hostName}上的 Host 太旧，连不上`,
      appOld: (hostName: string) => `这个 App 太旧，${hostName} 不接受`,
    },
    desc: {
      old: (hostVersion: string, minHost: string) =>
        `它装的是 ${hostVersion}，这个 App 要 ${minHost} 以上。`,
      update: '到那台电脑上更新 Host，装好后回来重试。',
      appOld: '更新 App 后再连。',
    },
    card: {
      mismatch: (hostName: string) => `${hostName} · 版本对不上`,
      subtitle: (hostName: string, osVersion: string | null) =>
        osVersion === null
          ? `${hostName} · Host`
          : `${hostName} ${osVersion} · Host`,
      osFallback: { mac: 'macOS', windows: 'Windows 11' },
    },
    row: {
      host: '它装的 Host',
      app: '这个 App',
      appRequires: (appVersion: string) => `App ${appVersion} 要求`,
      hostRequires: (hostName: string) => `${hostName} 要求`,
    },
    action: {
      updateApp: '去更新 App',
      retry: '已更新，重试',
    },
    meta: {
      idle: (appVersion: string) => `App ${appVersion}`,
    },
    updateUnavailable: '没能打开 App 更新页面',
  },
  login: {
    kicker: {
      mac: 'Mac 停在登录窗口',
      windows: '电脑停在登录界面',
    },
    title: (hostName: string) => `${hostName} 重启了，还没登录`,
    desc: (os: EndOs) =>
      os === 'macos'
        ? '登录前 Host 采集不到画面。Tailscale 在线，可以先用屏幕共享登录，登录后自动切回 Host。'
        : os === 'windows'
        ? '登录前 Host 采集不到画面。Tailscale 在线，可以先用远程桌面登录，登录后自动切回 Host。'
        : '登录前 Host 采集不到画面。Tailscale 在线，可以先用屏幕共享或远程桌面登录，登录后自动切回 Host。',
    /** Tailscale 关着时：不提它。 */
    descLan: (os: EndOs) =>
      os === 'macos'
        ? '登录前 Host 采集不到画面。电脑在线，可以先用屏幕共享登录，登录后自动切回 Host。'
        : os === 'windows'
        ? '登录前 Host 采集不到画面。电脑在线，可以先用远程桌面登录，登录后自动切回 Host。'
        : '登录前 Host 采集不到画面。电脑在线，可以先用屏幕共享或远程桌面登录，登录后自动切回 Host。',
    extra: (os: EndOs, latency: string) =>
      os === 'macos'
        ? `直连 ${latency} · 经 Tailscale 的屏幕共享，输入 Mac 的登录密码`
        : os === 'windows'
        ? `直连 ${latency} · 经 Tailscale 的远程桌面，输入 Windows 的登录密码`
        : `直连 ${latency} · 输入这台电脑的登录密码`,
    extraLan: (os: EndOs, latency: string) =>
      os === 'macos'
        ? `直连 ${latency} · 用屏幕共享，输入 Mac 的登录密码`
        : os === 'windows'
        ? `直连 ${latency} · 用远程桌面，输入 Windows 的登录密码`
        : `直连 ${latency} · 输入这台电脑的登录密码`,
    action: {
      login: (os: EndOs) =>
        os === 'macos'
          ? '用屏幕共享登录'
          : os === 'windows'
          ? '用远程桌面登录'
          : '用屏幕共享或远程桌面登录',
      retry: '已登录，重试',
    },
    illustration: {
      password: '输入密码',
      footerLogin: '示意图 · 登录前没有实时画面',
      online: (node: string) => `Tailscale 在线 · ${node}`,
      onlineLan: (node: string) => `在线 · ${node}`,
    },
  },
  filevault: {
    kicker: '磁盘还没解锁',
    title: (hostName: string) => `${hostName} 的磁盘还锁着`,
    desc: '它开了 FileVault，重启后要先输入密码解锁磁盘。解锁前什么都不启动，Tailscale 也连不上它。',
    descLan:
      '它开了 FileVault，重启后要先输入密码解锁磁盘。解锁前什么都不启动，远程也连不上它。',
    descSameLan: '你和它在同一局域网，可以试试通过 SSH 解锁。',
    sshMeta: '通过 SSH 解锁要这台 Mac 开着远程登录',
    action: {
      ssh: '通过 SSH 解锁磁盘',
      goUnlock: '我去 Mac 前解锁',
      goUnlockWindows: '我去电脑前解锁',
      retry: '已解锁，重试',
    },
    illustration: {
      passwordHint: '输入密码后才会启动 macOS',
      offline: (lanAddr: string) => `Tailscale 离线 · 局域网 ${lanAddr}`,
      offlineUnknown: 'Tailscale 离线',
      offlineLan: (lanAddr: string) => `离线 · 局域网 ${lanAddr}`,
      offlineUnknownLan: '离线',
      footer: '示意图 · Mac 那边大概是这样',
    },
    dialog: {
      title: '通过 SSH 解锁磁盘',
      body: '在终端里运行下面的命令，按提示输入这台 Mac 的登录密码：',
      command: (user: string, lanAddr: string) => `ssh ${user}@${lanAddr}`,
      copy: '复制命令',
      copied: '已复制命令',
    },
  },
} as const;
