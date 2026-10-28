import { common } from './common';

export const ipadHome = {
  a11y: {
    add: common.a11y.add,
    more: '更多操作',
    refresh: '刷新',
    details: '更多操作',
    liveSummary: (host: string, role: string) => `${host}，${role}`,
    liveCard: (host: string) => `当前会话中的 ${host}`,
    infoCell: (label: string, value: string, sub?: string) =>
      [label, value, sub]
        .filter(part => part !== undefined && part !== '')
        .join('，'),
  },
  title: '设备',
  addDevice: '添加设备',
  separator: ' · ',
  sidebar: {
    computers: '电脑',
    servers: '服务器',
    serverCount: (count: number) => `${count} 台 · SSH`,
    rowHint: '轻点两下查看详情',
    rowLabel: (name: string, subtitle: string) =>
      subtitle === '' ? name : `${name}，${subtitle}`,
  },
  empty: {
    title: '还没有设备',
    message: '在电脑上装好 Remote Host 后扫码配对，或按地址添加',
    addDevice: '添加设备',
    noMatch: '没有匹配的设备',
    loadFailedTitle: '读不到设备列表',
  },
  detail: {
    online: '在线',
    offline: '离线',
    terminal: '终端',
    more: '更多操作',
    displayPill: (index: number) => `显示器 ${index}`,
    previewLabel: (name: string, index: number) =>
      `控制 ${name} 的显示器 ${index}`,
    control: '控制',
    connect: '连接',
    backToSession: '回到会话',
  },
  info: {
    path: '连接路径',
    displays: '显示器',
    host: 'Host',
    lastConnection: '上次连接',
    neverConnected: '还没连过',
    pathPending: '连上后显示',
    displayCount: (count: number) => `${count} 台`,
    permissionsOk: '权限齐全',
    permissionsMissing: '缺少权限',
    screenPermission: '屏幕录制',
    accessibilityPermission: '辅助功能',
    permissionOn: '✓',
    permissionOff: '✗',
    today: (time: string) => `${common.time.today} ${time}`,
  },
} as const;
