/** Unix 微秒；原生 u64 由适配层转换为 number。 */
export type Micros = number;
export type NetPath =
  | 'tailnetDirect'
  | 'tailnetDerp'
  | 'intranet'
  | 'lan'
  | 'manual';
export type Transport = 'quic' | 'tcp';
export type DevicePlatform = 'ios' | 'ipados' | 'android';
export type Quality = 'auto' | 'clear' | 'smooth' | 'eco';
export type Role = 'controller' | 'viewer';
export type Permission = 'viewOnly' | 'control' | 'unattended';

// 本地目录模型；原生适配层负责 snake_case 字段转换。
export type Address = {
  kind: 'tailnet' | 'intranet' | 'lan';
  addr: string;
  port: number;
  cidr?: string;
};
export type HostDisplay = {
  id: number;
  name: string;
  widthPx: number;
  heightPx: number;
  primary: boolean;
};
export type HostSessionSummary = {
  deviceName: string;
  role: Role;
  since: Micros;
};
export type ConsoleState = {
  loggedIn: boolean;
  locked: boolean;
  loginWindow: boolean;
};
export type Computer = {
  hostId: string;
  name: string;
  node: string;
  os: 'macos' | 'windows';
  osVersion?: string;
  online: boolean;
  hostRunning: boolean;
  lastSeenAt: Micros;
  lastHeartbeatAt: Micros | null;
  hostVersion: string;
  features: number;
  fp: string;
  addresses: Address[];
  displays: HostDisplay[];
  permsOk: boolean;
  sessions: HostSessionSummary[];
  paired: boolean;
  update: { channel: 'stable' | 'beta'; available: string | null };
  thumbnailUrl: string;
  filevault: boolean;
  console: ConsoleState | null;
  /** 只给离线且 Host 不在运行的电脑；与 wakeHost 同一套规则（05 · 目录）。 */
  wake?: WakeHint | null;
};
/**
 * 本地目录 `Computer.features` 的位（与 rsp-core `features.rs` 同值）。
 * 超过 31 位时位运算会出错，所以 `hasHostFeature` 用除法取位。
 */
export const HOST_FEATURE_PAIR_PASSWORD = 2 ** 19;
export const HOST_FEATURE_PAIR_FILE = 2 ** 20;
export const HOST_FEATURE_VIRTUAL_DISPLAY = 2 ** 22;
export function hasHostFeature(features: number, bit: number): boolean {
  return Math.floor(features / bit) % 2 === 1;
}
export type WakeHint = {
  available: boolean;
  reason: 'no_wake_nic' | 'no_relay' | null;
  /** 中继是同一用户的 Host 时才有名字。 */
  relayName: string | null;
};
export type Server = {
  node: string;
  addr: string;
  online: boolean;
  os: string;
  tailscaleSsh: boolean | null;
};
/** tailnet 里没装 Host 的 Mac / Windows。 */
export type OtherDevice = {
  node: string;
  name: string;
  os: 'macos' | 'windows';
  addrs: string[];
  online: boolean;
  lastSeenAt: Micros | null;
};
export type Directory = {
  generatedAt: Micros;
  computers: Computer[];
  servers: Server[];
  others: OtherDevice[];
};
/**
 * `ready` 表示新版本已启动并通过自检（D32），与本地 IPC 安装前的 ready 不同。
 */
export type HostUpdateState =
  | 'idle'
  | 'waitingSessionEnd'
  | 'downloading'
  | 'verifying'
  | 'installing'
  | 'restarting'
  | 'ready'
  | 'rolledBack'
  | 'failed';
export type HostDetail = Omit<
  Computer,
  'permsOk' | 'paired' | 'update' | 'thumbnailUrl'
> & {
  arch: 'universal' | 'x64' | 'arm64' | null;
  osVersion: string;
  perms: { screen: boolean; accessibility: boolean };
  paired: string[]; // 已配对的 device_id
  update: {
    channel: 'stable' | 'beta';
    pending: string | null;
    state: HostUpdateState;
  };
  thumbnailUrl: string | null;
  uninstalledAt?: Micros | null;
};
export type SyncValue<T = unknown> = { version: number; value: T };

// 身份 / 配对（04）
export type Identity = { deviceId: string; fp: string };
/** rsp://pair?v=1&h=&fp=&c=&n=&a= */
export type PairTarget = {
  v: 1;
  hostId: string;
  fp: string;
  code: string;
  name: string;
  addrs: string[];
};
/** 配对文件摘要（04「其他配对方式」）；口令留在原生，不进 JS。 */
export type PairFileSummary = {
  fileId: string;
  hostId: string;
  hostName: string;
  fp: string;
  addrs: string[];
  expiresAt: Micros;
  expired: boolean;
};
export type PairStep = 'dialing' | 'verifying' | 'exchanging' | 'waitingLocal';
export type PairOutcome =
  | {
      status: 'ok';
      hostId: string;
      hostName: string;
      permission?: Permission;
      path?: NetPath;
      latencyMs?: number;
    }
  | { status: 'badCode'; attemptsLeft: number }
  /** retryAfterS：距解锁的秒数（Host 给出时才有）。 */
  | { status: 'locked'; retryAfterS?: number }
  | {
      status:
        | 'denied'
        | 'expired'
        | 'disabledOnPath'
        | 'methodDisabled'
        | 'mitm'
        | 'cancelled'
        | 'timeout';
    }
  | { status: 'error'; code?: number; message: string };
export type PairedHost = {
  hostId: string;
  fp: string;
  name: string;
  addresses: string[];
  pairedAt: Micros;
  /** V11：最后一次 ServerHello 记下的；没连过为 null。 */
  os: 'macos' | 'windows' | 'linux' | null;
  osVersion: string | null;
  hostVersion: string | null;
  features: number | null;
  displays: HostDisplay[];
  lastSeenAt: Micros | null;
};

// 会话（03 §3、§4、§10）；u64 ID 由 原生适配层 转为十进制字符串。
export type SessionId = string;
export type OpenOpts = {
  intent: 'control' | 'view';
  displayId?: number;
  quality?: Quality;
  mode?: 'trackpad' | 'touch';
  /** 握手时请求远端声音；不传 = 不请求。 */
  audio?: boolean;
  /** 本机屏幕最高刷新率；省略时 Rust 用 60（07 · RsCore 启动）。 */
  maxFps?: number;
  /** 期望的最大编码宽度（像素）；省略时 Rust 用 1920。手机 App 不传（07 · RsCore）。 */
  maxWidth?: number;
  /** 唯一的解码能力；省略时 HEVC。手机 App 不传。 */
  codec?: 'hevc' | 'h264';
};
/** 07 未定义；按 03 §4、§10 和画布 SessionConnecting 约定，待 07 补登。 */
export type SessionPhase =
  | 'connecting'
  | 'authenticating'
  | 'pendingApproval'
  | 'streaming'
  | 'reconnecting'
  | 'ended';
export type SessionSummary = {
  id: SessionId;
  hostId: string;
  hostName: string;
  phase: SessionPhase;
  role: Role | null;
  path: NetPath | null;
  transport: Transport | null;
  startedAt: Micros;
  displayId: number | null;
};
export type Participant = {
  deviceId: string;
  deviceName: string;
  user: string;
  path: NetPath | null;
  since: Micros;
};
export type SessionDisplay = {
  id: number;
  name: string;
  widthPx: number;
  heightPx: number;
  primary: boolean;
  refreshHz?: number;
  /** Host 建的虚拟屏为 true。 */
  isVirtual?: boolean;
};
export type VdisplayStatus =
  | 'ok'
  | 'unsupported'
  | 'notAllowed'
  | 'limit'
  | 'badRequest'
  | 'failed';
export type VirtualDisplayOpts = {
  widthPx: number; // 像素，保持横竖
  heightPx: number;
  scale?: number; // 不传 = Host 选
  refreshHz?: number; // 不传 = 60
  select?: boolean; // 建好后切过去；不传 = false
};
export type GoodbyeReason =
  | 'userClosed'
  | 'hostClosedByLocalUser'
  | 'idleTimeout'
  | 'replaced'
  | 'revoked'
  | 'background'
  | 'hostShutdown'
  | 'hostUpdating'
  | 'permissionLost'
  | 'protocolError';
/** 07 未定义；按 03 §10 和画布 SessionConnecting 约定，待 07 补登。 */
export type DialAttempt = {
  path: NetPath;
  addr: string;
  state: 'trying' | 'ok' | 'failed' | 'skipped';
  ms?: number;
  reason?: string;
};
export type SessionCombo =
  | 'cmdTab'
  | 'cmdSpace'
  | 'missionControl'
  | 'altTab'
  | 'winKey'
  | 'ctrlSpace'
  | 'cmdH'
  | 'cmdShift4'
  | 'winSpace';
export type SessionActionKind = 'lock' | 'ctrlAltDel' | 'lockOnEnd';

// 诊断
export type ProbeId = string;
export type ProbeOutcome =
  | 'ok'
  | 'timeout'
  | 'refused'
  | 'fingerprintMismatch'
  | 'noRoute'
  | 'needsVpn'
  | 'skipped';

// SSH
export type SshId = string;
export type SshAuth = 'tailscale' | 'deviceKey' | 'password';
export type SshServer = {
  serverId: string;
  name: string;
  host: string;
  port: number;
  user: string;
  jump: string | null;
  auth: SshAuth;
  tailscaleSsh: boolean | null;
  os?: string;
  source: 'directory' | 'manual';
};
export type SshKey = {
  keyId: string;
  name: string;
  kind: 'se-p256' | 'ed25519';
  fp: string;
  publicKey: string;
  createdAt: Micros;
  installedOn: string[]; // serverId
};
export type TrustedHostKey = {
  serverId: string;
  keyType: string;
  fp: string;
  addedAt: Micros;
};
export type SshOpenOpts = {
  tmux?: boolean;
  target?: {
    host: string;
    port: number;
    user: string;
    jump?: string | null;
    auth: SshAuth;
  };
};

// 网络（07 · 网络判定）；数据来自原生 NetInfoPlus（P8）。
export type NetIface = {
  name: string;
  addrs: string[];
  isVpn: boolean;
  workProfile?: boolean;
};
export type TailnetNodeState =
  | 'noState'
  | 'needsLogin'
  | 'needsMachineAuth'
  | 'stopped'
  | 'starting'
  | 'running'
  | 'unknown'
  | 'unavailable';
export type TailnetRoute = 'system' | 'embedded';
export type EmbeddedNodeSummary = {
  state: TailnetNodeState;
  hostname: string;
  ips: string[];
  derpRegion: string;
  derpRegionName: string;
};
export type TailnetRouteStatus = {
  mode: 'system' | 'embedded' | 'auto';
  active: TailnetRoute;
  available: boolean;
  controlUrlSet: boolean;
  embedded: EmbeddedNodeSummary | null;
};
export type NetworkStatus = {
  ifaces: NetIface[];
  tailnet?: TailnetRouteStatus;
};

// tailnet_status() 的 JSON，state 为 camelCase。
export type TailnetSelf = {
  hostname: string;
  dnsName: string;
  ips: string[];
  online: boolean;
  derpRegion: string;
  derpRegionName: string;
};
export type TailnetPeer = {
  hostname: string;
  dnsName: string;
  ips: string[];
  os: string;
  online: boolean;
  tags: string[];
  direct: boolean;
  curAddr: string;
  relay: string;
  peerRelay: string;
  lastHandshakeUs: number;
  rxBytes: number;
  txBytes: number;
};
export type TailnetStatus = {
  state: TailnetNodeState;
  authUrl: string | null;
  self: TailnetSelf | null;
  magicDnsSuffix: string;
  peers: TailnetPeer[];
};

// 内置 VNC / RDP 适配器。
export type AdapterProtocol = 'vnc' | 'rdp';
export type AdapterTarget = {
  protocol: AdapterProtocol;
  hostId?: string;
  addr: string;
  port?: number;
};
export type AdapterContext =
  | 'loginWindow'
  | 'noHost'
  | 'fallback'
  | 'manual'
  | 'native';
export type AdapterOpenOpts = {
  viewWidth: number;
  viewHeight: number;
  scale: number;
  monitors?: number;
  useSaved?: boolean;
  context?: AdapterContext;
};
export type AdapterPhase =
  | 'connecting'
  | 'needsCredentials'
  | 'authenticating'
  | 'streaming'
  | 'ended';
export type AdapterCredField = 'user' | 'password' | 'domain';
export type AdapterErrorCode =
  | 'refused'
  | 'timeout'
  | 'authFailed'
  | 'unsupportedSecurity'
  | 'nlaRequired'
  | 'certRejected'
  | 'protocol'
  | 'serverClosed'
  | 'io'
  | 'cancelled';
export type AdapterErrorInfo = {
  code: AdapterErrorCode;
  rsCode: number;
  message: string;
};
export type AdapterSummary = {
  sessionId: SessionId;
  target: AdapterTarget;
  phase: AdapterPhase;
  width?: number;
  height?: number;
};
/** `guess`：服务端没给布局、App 从画面推断出来的屏。 */
export type AdapterScreen = {
  x: number;
  y: number;
  width: number;
  height: number;
  guess?: 'blackGap' | 'aspect';
};
export type SavedCredentialInfo = { saved: boolean; user?: string; domain?: string };
export const ADAPTER_RS_E = {
  unreachable: 200,
  authFailed: 201,
  unsupported: 202,
  certRejected: 203,
  broken: 204,
  serviceOff: 205,
} as const;
export type AdapterCredentials = {
  user?: string;
  password: string;
  domain?: string;
  save: boolean;
};

// 优先使用操作系统原生协议。
export type DiscoverKind = 'vnc' | 'rdp' | 'tcp';
export type DiscoverOutcome = 'open' | 'mismatch' | 'refused' | 'timeout';
export type DiscoverProbe = {
  addr: string;
  port: number;
  outcome: DiscoverOutcome;
  ms: number;
  banner?: string;
};

// Tailscale API 只读令牌用于列出 tailnet 设备。令牌只进原生钥匙串，不回到 JS。
// 也支持自建 Headscale：令牌是它的 API key，控制服务器地址和令牌一起存。
/** Headscale 不报系统，它的设备一律 unknown（App 按开放端口分类）。 */
export type TailscaleOs =
  | 'linux'
  | 'macos'
  | 'windows'
  | 'ios'
  | 'android'
  | 'other'
  | 'unknown';
export type TailscaleDevice = {
  /** Tailscale 的 nodeId；Headscale 的节点 id。 */
  id: string;
  /** 机器名：dnsName 的第一段（如 aliyun-bj）；Headscale 取 givenName，没有时取 name。 */
  name: string;
  /** MagicDNS 全名，不带末尾的点（如 aliyun-bj.tail1234.ts.net）；没有时（含 Headscale）为空串。 */
  dnsName: string;
  os: TailscaleOs;
  /** IPv4 在前。 */
  addresses: string[];
  /** API 的 connectedToControl（Headscale 的 online）；没给时为 null。 */
  online: boolean | null;
  lastSeenAt: Micros | null;
  /** Headscale 是 validTags + forcedTags 去重。 */
  tags: string[];
};
export type TailscaleApiError = 'auth' | 'network' | 'http';
export type TailscaleApiStatus = {
  configured: boolean;
  /** Headscale 控制服务器（https://主机[:端口]，非机密）；官方 Tailscale 或没配置时为 null。 */
  controlUrl: string | null;
  /** 最近一次请求的结果；还没请求过为 null。 */
  lastError: TailscaleApiError | null;
  lastOkAt: Micros | null;
  deviceCount: number | null;
};

// 事件（07）；连接阶段和拨号详情按 03 §4、§10 / SessionConnecting 补全，待 07 补登。
// ---------- 会话录制（07「会话录制」）----------
export type RecordingId = string;
export type PlaybackId = string;
export type RecordingFileState = 'complete' | 'recovered' | 'recording' | 'damaged';
export type RecordingInfo = {
  recordingId: RecordingId;
  hostId: string;
  hostName: string;
  /** unix 微秒 */
  startedAt: number;
  durationMs: number;
  bytes: number;
  state: RecordingFileState;
  hasAudio: boolean;
  width: number;
  height: number;
};
export type RecordingStorage = {
  usedBytes: number;
  capBytes: number;
  freeBytes?: number;
  /** 缺省 = 永久保留 */
  retentionDays?: number;
};
export type RecordingList = { recordings: RecordingInfo[]; storage: RecordingStorage };
export type RecordingStatus = {
  sessionId: SessionId;
  recordingId?: RecordingId;
  state: 'idle' | 'recording' | 'stopping';
  bytes: number;
  durationMs: number;
};
export type PlaybackInfo = {
  playbackId: PlaybackId;
  recordingId: RecordingId;
  durationMs: number;
  width: number;
  height: number;
  hasAudio: boolean;
};
export type PlaybackCommand =
  | { op: 'play' }
  | { op: 'pause' }
  | { op: 'seek'; positionMs: number };
export type RecordingExport = { path: string; bytes: number; mime: string };

export type CoreEvent =
  | {
      type: 'discover.found';
      scanId: string;
      addr: string;
      port: number;
      banner?: string | null;
    }
  | {
      type: 'discover.done';
      scanId: string;
      scanned: number;
      cancelled: boolean;
    }
  | { type: 'file.transfer'; transfer: FileTransfer }
  | { type: 'file.transferRemoved'; transferId: string }
  | {
      type: 'session.state';
      sessionId: SessionId;
      hostId: string;
      phase: SessionPhase;
      path?: NetPath;
      transport?: Transport;
      derpRegion?: string;
      dial?: DialAttempt[];
      approvalTimeoutS?: number;
      resumed?: boolean;
      reconnectReason?: 'pathLost' | 'hostUpdating' | 'resume';
    }
  | {
      type: 'session.role';
      sessionId: SessionId;
      role: Role;
      controller: Participant | null;
      viewers: Participant[];
      inputPausedLocal: boolean;
    }
  | {
      type: 'session.stats';
      sessionId: SessionId;
      rttMs: number;
      fps: number;
      bitrateKbps: number;
      decodeMs: number;
      lossPct: number;
      width: number;
      height: number;
      codec: 'hevc' | 'h264';
      path: NetPath;
      transport: Transport;
      derpRegion?: string;
      rateReason?: string;
    }
  | {
      type: 'session.error';
      sessionId: SessionId;
      hostId: string;
      code?: number;
      reason?: GoodbyeReason;
      message?: string;
      detail?: Record<string, string>;
      retryAfterS?: number;
    }
  | {
      type: 'session.displays';
      sessionId: SessionId;
      displays: SessionDisplay[];
      currentId: number;
    }
  | {
      type: 'session.vdisplay';
      sessionId: SessionId;
      requestId: number;
      status: VdisplayStatus;
      display?: SessionDisplay;
      message?: string;
    }
  | {
      type: 'session.clipboard';
      sessionId: SessionId;
      direction: 'fromHost' | 'toHost';
      text: string;
      at: Micros;
    }
  // 03 已有 IdleWarning，画布 SessionIdle 需要；待 07 补登。
  | {
      type: 'session.idleWarning';
      sessionId: SessionId;
      secondsLeft: number;
      lockOnEnd: boolean;
    }
  // 03 已有 ControlRequest，画布 SessionShared 需要；待 07 补登。
  | {
      type: 'session.controlRequest';
      sessionId: SessionId;
      requester: Participant;
      timeoutS: number;
    }
  | {
      type: 'pairing.progress';
      step: PairStep;
      hostName?: string;
      path?: NetPath;
      latencyMs?: number;
      fp?: string;
      timeoutS?: number;
    }
  | { type: 'pairing.done'; outcome: PairOutcome }
  | {
      type: 'probe.result';
      probeId: ProbeId;
      hostId: string | null;
      path: NetPath;
      addr: string;
      port: number;
      transport: Transport;
      outcome: ProbeOutcome;
      ms: number | null;
      reason?: string;
      last: boolean;
    }
  | {
      type: 'ssh.state';
      sshId: SshId;
      serverId: string;
      state: 'connecting' | 'authenticating' | 'open' | 'closed' | 'failed';
      message?: string;
    }
  | {
      type: 'ssh.hostKeyPrompt';
      sshId: SshId;
      serverId: string;
      keyType: string;
      fp: string;
      kind: 'first' | 'changed';
      oldFp?: string;
    }
  | {
      type: 'ssh.authUrl';
      sshId: SshId;
      serverId: string;
      url: string;
      user: string;
    }
  | {
      type: 'tailnet.state';
      state: TailnetNodeState;
      /** needsLogin 时的登录地址；只用来打开浏览器，不得写日志、不得进诊断导出 */
      authUrl: string | null;
      ips: string[];
      derpRegion: string;
      message: string | null;
    }
  | {
      type: 'adapter.state';
      sessionId: SessionId;
      protocol: AdapterProtocol;
      phase: AdapterPhase;
      credFields?: AdapterCredField[];
      credError: boolean;
      width?: number;
      height?: number;
      serverName?: string;
      error?: AdapterErrorInfo;
    }
  | {
      type: 'adapter.certPrompt';
      sessionId: SessionId;
      fp: string;
      oldFp?: string;
    }
  | { type: 'adapter.clipboard'; sessionId: SessionId; text: string }
  /** The computer copied something other than text (a screenshot, files). */
  | { type: 'adapter.clipboardOther'; sessionId: SessionId }
  | { type: 'adapter.screens'; sessionId: SessionId; screens: AdapterScreen[] }
  | { type: 'network.changed'; status: NetworkStatus }
  | {
      type: 'recording.state';
      sessionId: SessionId;
      recordingId: RecordingId;
      state: 'recording' | 'stopped' | 'failed';
      bytes: number;
      durationMs: number;
      reason?: string;
    }
  | {
      type: 'playback.state';
      playbackId: PlaybackId;
      recordingId: RecordingId;
      state: 'loading' | 'playing' | 'paused' | 'ended' | 'error';
      positionMs: number;
      durationMs: number;
      error?: string;
    };
export type CoreEventType = CoreEvent['type'];
export type CoreEventOf<T extends CoreEventType> = Extract<
  CoreEvent,
  { type: T }
>;

// File browser and transfer types.
export type TransferId = string;
export type FileSourceRef =
  | { kind: 'computer'; hostId: string }
  | { kind: 'server'; serverId: string };
export type FileEntryKind = 'file' | 'dir' | 'symlink' | 'other';
export type FileEntry = {
  name: string;
  kind: FileEntryKind;
  size: number | null;
  mtimeUs: number | null;
  childCount: number | null;
};
export type FileListing = {
  path: string;
  parent: string | null;
  entries: FileEntry[];
  truncated: boolean;
  netPath: NetPath | null;
};
export type FileTransferDirection = 'upload' | 'download';
export type FileTransferState =
  | 'queued'
  | 'running'
  | 'paused'
  | 'interrupted'
  | 'verifying'
  | 'completed'
  | 'failed'
  | 'cancelled';
export type FileTransferFail =
  | 'io'
  | 'sha256Mismatch'
  | 'resumeMismatch'
  | 'sourceChanged'
  | 'protocol'
  | 'tooLarge'
  | 'notFound'
  | 'permissionDenied'
  | 'notAllowed'
  | 'noSpace'
  | 'authRequired'
  | 'disabled'
  | 'unsupported';
export type FileTransfer = {
  transferId: TransferId;
  direction: FileTransferDirection;
  source: FileSourceRef;
  name: string;
  remotePath: string | null;
  localPath: string | null;
  size: number;
  done: number;
  bytesPerSec: number;
  state: FileTransferState;
  fail: FileTransferFail | null;
  netPath: NetPath | null;
  savedName: string | null;
  startedAt: Micros;
  updatedAt: Micros;
};

export type CoreConfig = {
  dataDir: string;
  appVersion: string;
  platform: DevicePlatform;
  filesDir?: string;
};

/** 交给 RsCore.crash.recordJs 的 JS 致命错误。 */
export type JsCrashReport = {
  name: string | null;
  message: string;
  stack: string | null;
  isFatal: boolean;
  osVersion: string | null;
};
