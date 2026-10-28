// 会话提示的文案（只在这里，不建 i18n 聚合文件）。
import type { GoodbyeReason } from '@app/native/rscore/types';

const reasons = {
  hostClosedByLocalUser: '对方在那台电脑上结束了会话',
  idleTimeout: '长时间没有操作，已断开',
  replaced: '被另一台设备接管',
  revoked: '本机的访问已被撤销',
  hostShutdown: '那台电脑关机或重启了',
  hostUpdating: '那台电脑正在更新',
  permissionLost: '那台电脑丢失了屏幕录制或辅助功能权限',
  protocolError: '协议错误',
} satisfies Partial<Record<GoodbyeReason, string>>;

export const text = {
  ended: (hostName: string, reason: string | null) =>
    reason === null ? `与 ${hostName} 的连接已断开` : `与 ${hostName} 的连接已断开：${reason}`,
  unknownHost: '那台电脑',
  reasons,
  close: '关闭',
  controlRequest: (deviceName: string, user: string, seconds: number) =>
    `${deviceName}（${user}）请求控制，${seconds} 秒内回应`,
  allow: '允许',
  deny: '拒绝',
};
