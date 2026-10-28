export const devicePasscodeAuthName = '设备密码';

export const androidBiometryAuthNames = {
  fingerprint: '指纹',
  face: '人脸',
  iris: '虹膜',
  biometric: '生物识别',
} as const;

/** 是否为 Android 生物识别的中文名（不含「设备密码」）。 */
export function isAndroidAuthName(name: string): boolean {
  return Object.values(androidBiometryAuthNames).includes(
    name as (typeof androidBiometryAuthNames)[keyof typeof androidBiometryAuthNames],
  );
}

/** 中文认证名：「设备密码」加上 Android 生物识别名。拼进句子时两边不加空格。 */
export function isCjkAuthName(name: string): boolean {
  return devicePasscodeAuthName === name || isAndroidAuthName(name);
}

export const appLock = {
  gate: {
    title: 'Remote Screen 已锁定',
    subtitle: '解锁后回到你离开的地方',
    sessionTitle: (remote: number, terminals: number) => {
      const parts: string[] = [];
      if (remote > 0) {
        parts.push(`${remote} 个远控会话`);
      }
      if (terminals > 0) {
        parts.push(`${terminals} 个终端`);
      }
      return `有 ${parts.join('、')}在后台保持`;
    },
    sessionInfo:
      '锁定时不显示远端画面 · 切到后台 30 秒后暂停，1 分钟内回来都能接上',
    unlock: (name: string) =>
      isCjkAuthName(name) ? `用${name}解锁` : `用 ${name} 解锁`,
    devicePasscodeHint: '验证弹窗里也可以改用设备密码',
    authenticateReason: '解锁 Remote Screen',
    failed: '没有验证通过，再试一次',
  },
  screen: {
    back: '返回设置',
    title: (name: string) =>
      isCjkAuthName(name) ? `${name}解锁` : `${name} 解锁`,
    description: (name: string) =>
      isCjkAuthName(name)
        ? `用${name}保护这台手机上的远控入口。验证在手机上完成，不会上传。`
        : `用 ${name} 保护这台手机上的远控入口。验证在手机上完成，不会上传。`,
    onOpenTitle: '打开 App 时验证',
    onOpenSubtitle: '从后台回来超过 5 分钟也要验证',
    beforeSessionTitle: '进入远控前验证',
    beforeSessionSubtitle: '每次连接电脑、打开终端前都要验证',
    sensitiveTitle: '敏感电脑',
    sensitiveRight: '连接前总要验证',
    sensitiveSwitchLabel: (name: string) =>
      isAndroidAuthName(name) ? `${name}连接前验证` : `${name} 连接前验证`,
    emptyHosts: '还没有电脑',
    alwaysTitle: 'SSH 私钥',
    alwaysValue: '存在安全芯片里，导不出来',
    disableFailed: '没有验证通过，保护没有关',
    authenticateDisableReason: (name: string) =>
      isCjkAuthName(name) ? `关闭${name}保护` : `关闭 ${name} 保护`,
  },
} as const;
