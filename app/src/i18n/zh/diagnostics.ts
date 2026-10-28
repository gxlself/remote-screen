import { common } from './common';

const REASON: Record<string, string> = {
  timeout: '超时',
  refused: '被拒绝',
  noRoute: '没有路由',
  needsVpn: '需要公司 VPN',
  fingerprintMismatch: '证书指纹不符',
  skipped: '跳过',
  holePunchFailed: '打洞失败',
};

function reasonLabel(reason: string | null): string {
  if (reason === null || reason === '') {
    return REASON.timeout;
  }
  return REASON[reason] ?? reason;
}

function reasonText(
  reason: string | null,
  ms: number | null,
  separator: string,
): string {
  const parts = [reasonLabel(reason)];
  if (ms !== null && Number.isFinite(ms)) {
    parts.push(`${(ms / 1000).toFixed(1)} s`);
  }
  return parts.join(separator);
}

export const diagnostics = {
  title: '诊断',
  back: common.back,
  backDestination: common.settings,
  a11y: {
    changeHint: (name: string) => `现在测的是 ${name}`,
    pickerHint: (subtitle: string, selected: boolean) =>
      selected ? `${subtitle}，当前选中` : subtitle,
  },
  connection: {
    title: '连接测试',
    runningRight: '正在测，约 10 秒',
    finishedRight: (time: string, count: number) =>
      `${time} 测完 · ${count} 条路`,
    change: '换一台',
    pickerTitle: '选一台电脑',
    empty: '还没有电脑',
    emptyMessage: '先在电脑上装好 Remote Host，再回来测试',
    offline: '离线',
    listLabel: '逐条路测试结果',
    subtitle: (node: string, hostVersion: string) =>
      `${node} · Host ${hostVersion}`,
    pending: '测试中',
    fail: '不通',
    names: {
      tailnetDirect: 'tailnet 直连',
      tailnetDerp: 'DERP 中继',
      tailnetDerpRegion: (region: string) => `DERP ${region}`,
      intranetQuic: '内网 UDP 7400',
      intranetTcp: '内网 TCP 7400',
      lanQuic: '局域网 UDP 7400',
      lanTcp: '局域网 TCP 7400',
      manualQuic: '手动地址',
      manualTcp: '手动地址',
    },
    derpViaRelay: '经中继转发',
    reason: (reason: string | null, ms: number | null) =>
      reasonText(reason, ms, ' · '),
    reasonExport: (reason: string | null, ms: number | null) =>
      reasonText(reason, ms, '，'),
    a11y: (name: string, addr: string, detail: string) =>
      `${name}，${addr}，${detail}`,
  },
  advice: {
    running: '每条路最多等 3 秒，测完给出建议。',
    best: (name: string, ms: string) => `会走${name}，${ms} 最快。`,
    tcpMode: '公司网络拦了 UDP，所以是「TCP 模式」。',
    allFailed: '每条路都不通。看看电脑是否在线、Remote Host 是否在运行。',
  },
  actions: {
    stop: '停止',
    retest: '重新测试',
    copy: '拷贝结果',
    copied: common.copied,
  },
  exportText: {
    title: (name: string, node: string, time: string) =>
      `Remote Screen 诊断 · ${name}（${node}）· ${time}`,
    okLine: (name: string, addr: string, ms: string) =>
      `${name} ${addr}：${ms}`,
    failLine: (name: string, addr: string, reason: string) =>
      `${name} ${addr}：不通（${reason}）`,
    local: (text: string) => `本机 · ${text}`,
  },
  logs: {
    title: '日志',
    localLogs: '本机日志',
    localLogsSubtitle: '只存在手机上，保留 7 天',
    export: '导出 zip',
    exportTitle: 'Remote Screen 日志',
    exportFailed: (message: string) => `导出失败：${message}`,
    maskTitle: '导出时地址打码',
    maskSubtitle: 'IP 变成 10.30.•.• 这样，主机名也隐去',
    maskA11y: '导出时地址打码',
  },
  localNetwork: {
    label: '本机',
    companyVpn: (addr: string) => `公司 VPN ${addr}`,
    tailnet: (addr: string) => `tailnet ${addr}`,
    embedded: (addr: string) => `App 内 ${addr}`,
    unknown: '网络状态未知',
  },
  footer: (appVersion: string, minHostVersion: string) =>
    `Remote Screen ${appVersion} · RSP/1 · Host ≥ ${minHostVersion}`,
} as const;
