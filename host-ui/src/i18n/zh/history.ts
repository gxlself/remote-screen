const history = {
  title: '连接记录',
  description: '只保存在这台 Mac 上',
  columns: {
    time: '时间',
    device: '设备',
    event: '事件',
    path: '路径',
    duration: '时长',
  },
  empty: '还没有连接记录',
  errorPrefix: '读不到连接记录：',
  loadMore: '加载更多',
  deviceFallback: '—',
  events: {
    'session.start': '开始连接',
    'session.end': '结束连接',
    'auth.decision': '连接鉴权',
    'path.switch': '换路',
    'trust.apply': '执行作废 / 信任变更',
    upgrade: '软件更新',
    security: '安全事件',
  },
} as const;

export default history;
