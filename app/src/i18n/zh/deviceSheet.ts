import { isCjkAuthName } from './appLock';

export const deviceSheet = {
  title: '设备详情',
  displays: {
    section: '选择显示器',
    label: (index: number) => `显示器 ${index}`,
    primary: '主显示器',
    portrait: '竖放',
    control: (name: string) => `控制「${name}」`,
    start: '开始控制',
  },
  offline: {
    button: '电脑离线',
    reason: '等这台电脑开机联网后再连，App 还不能远程唤醒它。',
  },
  chips: {
    host: 'Host',
  },
  quickActions: {
    viewOnly: '仅观看',
    terminal: '终端',
    lock: '锁定屏幕',
    locked: '已锁定',
    offlineHint: '电脑离线，暂时用不了',
  },
  rows: {
    routes: '连接地址',
    lastConnection: '上次连接',
    addressCount: (count: number) => `${count} 条地址`,
  },
  switches: {
    sensitive: (name: string) =>
      isCjkAuthName(name) ? `连接前验证${name}` : `连接前验证 ${name}`,
    lockOnEnd: '会话结束后锁定远端屏幕',
  },
  pair: {
    cancel: '移除这台电脑',
    title: (name: string) => `移除 ${name}？`,
    message: '这台手机会忘掉它，之后要重新配对才能连接',
    confirm: '移除这台电脑',
    keep: '保留',
    failure: '没能移除，稍后再试',
  },
  native: {
    removed: (name: string) => `已删除 ${name}`,
    undo: '撤销',
    undoFailed: '没能恢复，稍后再试',
  },
  notFound: {
    title: '找不到这台设备',
  },
  a11y: {
    displayOption: (title: string, detail: string) => `${title}，${detail}`,
    lockNeedsSession: '有会话进行中时才能锁定',
  },
} as const;
