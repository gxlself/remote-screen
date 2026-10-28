/** 「Mac」两边留空格，「电脑」不留：在 Mac 上 / 在电脑上。 */
const mid = (host: string) => (host === '电脑' ? host : ` ${host} `);
const lead = (host: string) => (host === '电脑' ? host : `${host} `);

export const pairing = {
  steps: {
    connect: {
      title: '连上电脑',
    },
    verify: {
      title: '证书指纹一致',
      detail: '和 Mac 屏幕上的一致',
    },
    approve: {
      title: '等待你在 Mac 上点「允许」',
      detail: '确认窗口已经弹在 Mac 屏幕正中间',
    },
  },
  permissions: {
    viewOnly: {
      title: '仅观看',
      detail: '只看画面，不能动鼠标键盘',
    },
    control: {
      title: '允许控制',
      detail: '鼠标键盘、文件、剪贴板都能用',
    },
    unattended: {
      title: '无人值守',
      detail: '勾上「允许无人值守」，以后连接不用再去点',
    },
  },
  failure: {
    badCode: {
      title: '配对码不对',
      invalidCode: (host: string) =>
        `这个码已失效，在${mid(host)}上重新显示配对码`,
    },
    expired: {
      title: '配对码已过期',
      message: (host: string, method: 'qr' | 'code') =>
        method === 'qr'
          ? `二维码 5 分钟内有效，只能用一次。在${mid(host)}上重新显示二维码，再扫一次`
          : `配对码 5 分钟内有效，只能用一次。在${mid(host)}上重新显示配对码`,
    },
    denied: {
      title: (host: string) => `${lead(host)}上拒绝了`,
      message: (hostName: string) => `${hostName} 前的人点了「拒绝」`,
    },
    timeout: {
      title: '没人点「允许」',
      message: (host: string, method: 'qr' | 'code') =>
        method === 'qr'
          ? `${lead(host)}已自动拒绝，重新扫码就行`
          : `${lead(host)}已自动拒绝，重新输入配对码就行`,
    },
    locked: {
      title: '尝试太多次',
      message: '这台电脑暂时不接受新的配对，过几分钟再试',
      retryIn: (remaining: string) => `请 ${remaining}后再试`,
    },
    disabledOnPath: {
      title: '这条路不允许配对',
      message: '这台电脑没打开「允许新设备经内网配对」，连上 Tailscale 再试',
      /** Tailscale 关着时。 */
      messageLan: '这台电脑没打开「允许新设备经内网配对」，在电脑的 Host 里打开后再试',
    },
    mitm: {
      title: '身份校验失败',
      message: '对方的证书和二维码里的不一致，可能连到了另一台电脑。已停止配对',
    },
    error: {
      title: '配对没有完成',
      fallbackMessage: '出了点问题，稍后再试',
    },
    methodDisabled: {
      title: '这台电脑没打开这种配对方式',
      message: '改用配对码配对',
    },
    file: {
      expired: {
        title: '配对文件已失效',
        message: '文件已用过、被撤销或已过期。在电脑上重新导出一份',
      },
      badCode: {
        title: '配对文件校验失败',
        attempts: (n: number) => `文件可能被改过，还能再试 ${n} 次`,
        exhausted: '文件可能被改过，在电脑上重新导出一份',
      },
      notLoaded: {
        title: '请重新导入配对文件',
        message: '配对文件没有加载成功，请重新选择',
      },
      hostTooOld: {
        title: '这台电脑上的 Host 版本太旧，不认识配对文件',
        message: '升级后才能用配对文件配对，现在可以先用配对码',
      },
    },
  },
  actions: {
    reenterCode: '重新输入配对码',
    rescan: '重新扫码',
    reimportFile: '导入别的文件',
    otherMethod: '换一种方式',
  },
  attemptsLeft: (n: number) => `还能再试 ${n} 次`,
  retryAfter: {
    seconds: (n: number) => `${n} 秒`,
    minutes: (n: number) => `${n} 分钟`,
  },
  /** 不知道系统时叫「电脑」，不能把 Windows 叫成 Mac。 */
  hostWord: (os: 'macos' | 'windows' | null) =>
    os === 'macos' ? 'Mac' : '电脑',
} as const;
