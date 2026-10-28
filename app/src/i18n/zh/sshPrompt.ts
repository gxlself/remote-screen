import { isCjkAuthName } from './appLock';

export const sshPrompt = {
  hostKey: {
    firstTitle: (name: string) => `第一次连接 ${name}`,
    firstBody:
      '核对一下主机指纹，和在服务器上运行 ssh-keygen -lf 看到的一致再继续。',
    changedTitle: (name: string) => `${name} 的主机指纹变了`,
    changedBody:
      '可能是服务器重装了系统，也可能有人在中间冒充它。确认是你们自己换的再继续。',
    original: '原来',
    current: '现在',
    originalMissing: '没有记下原来的指纹',
    fingerprint: (keyType: string, spoken: string) =>
      `主机指纹 ${keyType}，${spoken}`,
    trustAndConnect: '信任并连接',
    disconnect: '断开',
    trustChanged: '我确认换过了，信任新指纹',
    authReason: '确认信任新的主机指纹',
    authFailed: '没有验证通过，新指纹没有保存',
    saveFailed: (message: string) => `没能保存指纹：${message}`,
    noLockDialog: {
      title: '再确认一次',
      message:
        '这台手机没设锁屏密码，没法用它验证。确定服务器换过主机密钥，再信任新指纹。',
      confirm: '信任新指纹',
    },
  },
  rootConfirm: {
    title: (user: string, name: string) =>
      `以 ${user} 登录 ${name} 需要再确认一次`,
    body: (user: string) =>
      `Headscale 策略对 ${user} 开了二次确认（check 模式）。确认后一段时间内不用再确认，用其他账号登录不受影响。`,
    loginAs: '登录为',
    loginValue: (user: string, name: string) => `${user}@${name}`,
    thisDevice: '本机',
    stepsLabel: '确认步骤',
    steps: {
      faceIdAsk: (name: string) =>
        isCjkAuthName(name)
          ? `用${name}确认是你本人`
          : `用 ${name} 确认是你本人`,
      faceIdDone: (name: string) =>
        isCjkAuthName(name) ? `${name}已确认` : `${name} 已确认`,
      browser: (host: string, waiting: boolean) =>
        waiting ? `在浏览器登录 ${host} · 等你登录` : `在浏览器登录 ${host}`,
      backToTerminal: '浏览器登录完成后会自动回到终端',
    },
    authReason: (user: string, name: string) => `确认以 ${user} 登录 ${name}`,
    confirm: (name: string) =>
      isCjkAuthName(name)
        ? `用${name}确认，并在浏览器登录`
        : `用 ${name} 确认，并在浏览器登录`,
    reopen: '浏览器没打开？再打开一次',
    openFailed: '打不开浏览器，请复制链接手动打开',
    copyLink: '复制链接',
  },
} as const;
