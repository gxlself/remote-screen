import type {
  AdapterErrorCode,
  AdapterProtocol,
} from '../../native/rscore/types';

const tool = { vnc: '屏幕共享', rdp: '远程桌面' } as const;

export const adapters = {
  entry: {
    vnc: '用屏幕共享打开',
    rdp: '用远程桌面打开',
    hint: (protocol: AdapterProtocol) =>
      protocol === 'vnc'
        ? '不经过 Host，直接连这台 Mac 自带的屏幕共享'
        : '不经过 Host，直接连这台电脑自带的远程桌面',
  },
  title: (protocol: AdapterProtocol) => tool[protocol],
  authenticate: (name: string) => `验证身份后连接 ${name}`,
  connecting: (name: string) => `正在连接 ${name}…`,
  authenticating: '正在登录…',
  loadingPicture: '正在加载画面…',
  waited: (seconds: number) =>
    seconds < 60
      ? `已等 ${seconds} 秒`
      : `已等 ${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`,
  cancel: '取消',
  unencrypted: '这条连接不经过 Tailscale，画面不加密。只在可信的网络里用。',
  /** Tailscale 关着时：不提它。 */
  unencryptedLan: '这条连接画面不加密，只在可信的网络里用。',
  unencryptedBadge: '不加密',
  unencryptedBadgeA11y: '画面不加密：这条连接不经过 Tailscale',
  unencryptedBadgeA11yLan: '画面不加密',
  rdpTakeover: '连上后，电脑本机的屏幕会锁上；在电脑前的人要重新登录。',
  creds: {
    title: (protocol: AdapterProtocol) =>
      protocol === 'vnc' ? '登录这台 Mac' : '登录这台电脑',
    intro: (protocol: AdapterProtocol, passwordOnly: boolean) =>
      protocol === 'rdp'
        ? '输入这台电脑的 Windows 账户和密码。Microsoft 账户填邮箱。'
        : passwordOnly
        ? '输入在「屏幕共享」设置里设的 VNC 密码。'
        : '输入这台 Mac 上的用户名和登录密码。',
    user: '用户名',
    password: '密码',
    domain: '域（可不填）',
    remember: '在这台设备上记住',
    submit: '登录',
    cancel: '取消',
    failed: (passwordOnly: boolean) =>
      passwordOnly ? '密码不对，再试一次。' : '用户名或密码不对，再试一次。',
    submitFailed: '没交上去，连接可能断了。再输一次密码试试，或者取消后重连。',
    invalidEmpty: '必填',
    invalidLength: (max: number) => `最长 ${max} 字节`,
    invalidControl: '不能包含换行或控制字符',
  },
  cert: {
    title: '这台电脑的证书变了',
    message:
      '和上次连接时记下的证书不一样。电脑重装系统或换了证书会这样；如果你没动过它，可能有人在冒充它。',
    oldFp: '上次',
    newFp: '这次',
    countdown: (seconds: number) => `${seconds} 秒内不确认，会自动断开`,
    trust: '信任新证书并继续',
    reject: '断开',
  },
  toolbar: {
    keyboard: '打开键盘',
    keyboardHide: '收起键盘',
    sendClipboard: '发送剪贴板',
    typePassword: '填入密码',
    mode: (mode: 'trackpad' | 'touch') =>
      `操作方式：${mode === 'trackpad' ? '触控板' : '触屏'}`,
    screens: '显示器',
    more: '更多',
    collapse: '收起工具栏',
    expand: '展开工具栏',
    dragHint: '按住拖到左边、右边或底部',
    move: {
      left: '移到左边',
      right: '移到右边',
      bottom: '移到底部',
    },
    disconnect: '断开',
  },
  disconnect: {
    title: (name: string) => `断开 ${name}？`,
    message: '电脑上的程序会继续运行，下次连上还在。',
    confirm: '断开',
    cancel: '取消',
  },
  reconnect: {
    title: '正在重连…',
    attempt: (attempt: number, max: number) => `第 ${attempt} 次，共 ${max} 次`,
    cancel: '取消',
  },
  more: {
    title: '更多',
    ctrlAltDel: 'Ctrl+Alt+Del',
    ctrlAltDelHint: '打开 Windows 的安全选项',
    sound: '播放电脑的声音',
    remoteClipboard: '复制电脑上的剪贴板',
    remoteClipboardHint: '自动同步关着，电脑上复制的内容在这里取',
    gestures: '手势说明',
    fitScreen: '适应屏幕',
    fitScreenHint: '取消放大，整个画面放回屏幕里',
    forget: '忘记保存的密码',
  },
  forget: {
    title: '忘记保存的密码？',
    message: '下次连这台电脑要重新输入密码。',
    confirm: '忘记',
    cancel: '取消',
  },
  toast: {
    clipboardSent: '已发送到电脑的剪贴板',
    clipboardEmpty: '手机剪贴板是空的',
    clipboardLatin1Only: '屏幕共享的剪贴板只支持西文字符',
    pasteLatin1Only: '屏幕共享只能粘贴英文和符号，中文请在电脑上输入',
    clipboardUnsupported: '这台电脑没开剪贴板共享',
    clipboardFailed: '剪贴板没发出去，再试一次',
    clipboardReceived: '已收到电脑的剪贴板',
    remoteClipboardCopied: '已复制到手机',
    passwordTyped: '已填入密码，按回车登录',
    passwordTypeFailed: '密码没填进去，再试一次',
    ctrlAltDelSent: '已发送 Ctrl+Alt+Del',
    ctrlAltDelFailed: 'Ctrl+Alt+Del 没发出去，再试一次',
    comboFailed: '快捷键没发出去，再试一次',
    screenSwitchFailed: '没切过去，再试一次',
    screenGone: '那台显示器不在了，已切回主显示器',
    soundFailed: '电脑的声音放不出来',
    forgotten: '已忘记保存的密码',
    forgetFailed: '没忘掉，再试一次',
    codeCopied: '已复制错误码',
  },
  errors: {
    title: (
      code: AdapterErrorCode,
      name: string,
      protocol: AdapterProtocol = 'vnc',
    ): string => {
      switch (code) {
        case 'refused':
          return protocol === 'vnc'
            ? `${name} 没打开屏幕共享`
            : `${name} 没打开远程桌面`;
        case 'timeout':
          return `连不上 ${name}`;
        case 'authFailed':
          return '登录失败';
        case 'unsupportedSecurity':
          return '不支持这台电脑的认证方式';
        case 'nlaRequired':
          return '这台电脑要求先登录';
        case 'certRejected':
          return '已断开';
        case 'serverClosed':
          return '电脑断开了连接';
        default:
          return '连接中断';
      }
    },
    /** `tailnetEnabled` false: Tailscale is not named (Settings › 外网访问). */
    message: (
      code: AdapterErrorCode,
      protocol: AdapterProtocol,
      tailnetEnabled = true,
    ): string => {
      switch (code) {
        case 'refused':
          return protocol === 'vnc'
            ? '这台 Mac 在线，但没打开屏幕共享。在 Mac 上打开「系统设置 › 通用 › 共享 › 屏幕共享」后重试，不用装任何东西。'
            : '这台电脑在线，但没打开远程桌面。在电脑上打开「设置 › 系统 › 远程桌面」后重试，不用装任何东西。';
        case 'timeout':
          return tailnetEnabled
            ? '电脑可能没开机、在睡眠，或者手机和它不在同一个网络（也没连上 Tailscale）。'
            : '电脑可能没开机、在睡眠，或者手机和它不在同一个网络。连上公司 VPN 或回到同一 Wi-Fi 再试。';
        case 'authFailed':
          return '试了三次都没登上。检查用户名和密码后重试。';
        case 'unsupportedSecurity':
          return protocol === 'vnc'
            ? '这台 Mac 只允许本 App 不支持的认证方式。可以换用其他屏幕共享 App。'
            : '这台电脑只允许本 App 不支持的安全协议。可以换用 Windows App。';
        case 'nlaRequired':
          return '它开着「网络级别身份验证」，要先输入 Windows 账户和密码。';
        case 'certRejected':
          return '你没有信任这台电脑的新证书。';
        case 'serverClosed':
          return '可能是电脑上有人结束了共享，或者别的设备接管了这台电脑。';
        default:
          return '和电脑的连接出了问题。';
      }
    },
    retry: '重试',
    retryWithPassword: '输入账户密码重试',
    back: '返回',
    otherApp: '用其他 App 打开',
    unavailableTitle: '这个版本还不能直接连',
    unavailableMessage:
      '这个安装包没带屏幕共享和远程桌面。更新 App 后再试，现在可以先用其他 App 打开。',
    certTimeoutTitle: '没来得及确认，已断开',
    certTimeoutMessage:
      '电脑的证书变了，60 秒内没有确认，为了安全先断开了。重试后再确认一次。',
  },
  screens: {
    title: '选择显示器',
    all: '全部显示器',
    item: (n: number) => `显示器 ${n}`,
    primary: '主显示器',
    /** 从画面推断出来的屏按位置命名。 */
    guessed: {
      left: '左侧显示器',
      right: '右侧显示器',
      top: '上方显示器',
      bottom: '下方显示器',
      middle: '中间显示器',
      fromLeft: (n: number) => `左起第 ${n} 台显示器`,
      fromTop: (n: number) => `上起第 ${n} 台显示器`,
    },
    aspectNote: '按画面比例拆分，可能不完全对齐',
  },
  /** 远程桌面会话里声明几台显示器。 */
  monitors: {
    title: '显示器数量',
    note: '远程桌面里的显示器，不一定对应电脑上的物理屏幕',
    value: (n: number) => `${n} 台`,
    reconnecting: (n: number) => `正在用 ${n} 台显示器重新连接…`,
    changeFailed: '显示器数量没改成，再试一次',
  },
  hostReady: {
    message: (name: string) => `${name} 上的 Host 已经可以用了`,
    action: '切换过去',
  },
  a11y: {
    view: (name: string) => `${name} 的远程画面`,
    credsSheet: '登录面板',
    toolbar: '远程控制工具栏',
    screens: (n: number) => `显示器，共 ${n} 台`,
  },
} as const;
