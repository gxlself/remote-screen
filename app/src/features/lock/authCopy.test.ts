import { appLock } from '../../i18n/zh/appLock';
import { deviceSheet } from '../../i18n/zh/deviceSheet';
import { onboarding } from '../../i18n/zh/onboarding';
import { pairWaiting } from '../../i18n/zh/pairWaiting';
import { settings } from '../../i18n/zh/settings';
import { sshPrompt } from '../../i18n/zh/sshPrompt';

function copy(name: string): string[] {
  return [
    appLock.gate.unlock(name),
    appLock.screen.title(name),
    appLock.screen.description(name),
    appLock.screen.authenticateDisableReason(name),
    settings.security.faceId.title(name),
    onboarding.protect.faceId.title(name),
    onboarding.protect.faceId.authReason(name),
    deviceSheet.switches.sensitive(name),
    pairWaiting.pairDone.sensitive.title(name),
    sshPrompt.rootConfirm.steps.faceIdAsk(name),
    sshPrompt.rootConfirm.steps.faceIdDone(name),
    sshPrompt.rootConfirm.confirm(name),
    appLock.screen.sensitiveSwitchLabel(name),
    onboarding.protect.faceId.unavailable(name),
  ];
}

it.each([
  [
    '设备密码',
    [
      '用设备密码解锁',
      '设备密码解锁',
      '用设备密码保护这台手机上的远控入口。验证在手机上完成，不会上传。',
      '关闭设备密码保护',
      '用设备密码解锁',
      '设备密码解锁',
      '开启设备密码解锁',
      '连接前验证设备密码',
      '连接前验证设备密码',
      '用设备密码确认是你本人',
      '设备密码已确认',
      '用设备密码确认，并在浏览器登录',
      '设备密码 连接前验证',
      '这台设备没有设置 设备密码 或密码',
    ],
  ],
  [
    'Face ID',
    [
      '用 Face ID 解锁',
      'Face ID 解锁',
      '用 Face ID 保护这台手机上的远控入口。验证在手机上完成，不会上传。',
      '关闭 Face ID 保护',
      '用 Face ID 解锁',
      'Face ID 解锁',
      '开启 Face ID 解锁',
      '连接前验证 Face ID',
      '连接前验证 Face ID',
      '用 Face ID 确认是你本人',
      'Face ID 已确认',
      '用 Face ID 确认，并在浏览器登录',
      'Face ID 连接前验证',
      '这台设备没有设置 Face ID 或密码',
    ],
  ],
  [
    'Touch ID',
    [
      '用 Touch ID 解锁',
      'Touch ID 解锁',
      '用 Touch ID 保护这台手机上的远控入口。验证在手机上完成，不会上传。',
      '关闭 Touch ID 保护',
      '用 Touch ID 解锁',
      'Touch ID 解锁',
      '开启 Touch ID 解锁',
      '连接前验证 Touch ID',
      '连接前验证 Touch ID',
      '用 Touch ID 确认是你本人',
      'Touch ID 已确认',
      '用 Touch ID 确认，并在浏览器登录',
      'Touch ID 连接前验证',
      '这台设备没有设置 Touch ID 或密码',
    ],
  ],
] as const)('keeps the existing copy for %s', (name, expected) => {
  expect(copy(name)).toEqual(expected);
});

it('uses compact Chinese copy for Android biometric names', () => {
  expect(copy('指纹')).toEqual([
    '用指纹解锁',
    '指纹解锁',
    '用指纹保护这台手机上的远控入口。验证在手机上完成，不会上传。',
    '关闭指纹保护',
    '用指纹解锁',
    '指纹解锁',
    '开启指纹解锁',
    '连接前验证指纹',
    '连接前验证指纹',
    '用指纹确认是你本人',
    '指纹已确认',
    '用指纹确认，并在浏览器登录',
    '指纹连接前验证',
    '这台设备没有设置生物识别或密码',
  ]);

  for (const name of ['人脸', '生物识别']) {
    expect(copy(name).every(value => !value.includes(' '))).toBe(true);
  }
});
