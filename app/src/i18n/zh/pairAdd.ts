export const pairAdd = {
  title: '配对电脑',
  methodLabel: '配对方式',
  scan: {
    tab: '扫码配对',
    hint: '对准电脑屏幕上的配对二维码',
    instructions:
      '在电脑菜单栏或托盘的 Host 里点「显示配对二维码」，再对准屏幕上的二维码。',
    pasteLabel: '粘贴二维码内容（开发用）',
    recognize: '识别',
    invalid: '这不是 Remote Screen 的配对码',
    cameraDenied: '没有相机权限',
    cameraDeniedHint: '请在系统设置中允许 Remote Screen 使用相机。',
    openSettings: '去设置',
    cameraUnavailable: '这台手机没有可用的相机，改用配对码吧',
    useCode: '输入配对码',
    cameraError: '相机没能启动',
    retry: '重试',
    torch: {
      open: '打开手电筒',
      close: '关闭手电筒',
    },
  },
  code: {
    tab: '输入配对码',
  },
  byAddress: {
    addrLabel: '电脑地址',
    addrPlaceholder: '100.64.0.3 或 home-mac',
    codeLabel: '配对码',
    codeHint: '在电脑菜单栏或托盘的 Host 里点「显示配对二维码」，窗口里有 6 位配对码。',
    start: '配对',
  },
  links: {
    byAddress: '按地址添加',
    sshServer: '添加 SSH 服务器',
  },
  other: {
    title: '其他配对方式',
    importPairFile: '导入配对文件',
  },
  a11y: {
    codeInput: '配对码输入',
    codeDigit: (position: number) => `配对码第 ${position} 位`,
    emptyDigit: '空',
    scanner: '扫码取景区域',
  },
} as const;
