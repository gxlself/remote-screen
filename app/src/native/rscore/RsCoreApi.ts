import type {
  AdapterCredentials,
  AdapterOpenOpts,
  AdapterSummary,
  AdapterTarget,
  CoreEvent,
  DevicePlatform,
  DiscoverKind,
  DiscoverProbe,
  FileListing,
  FileTransfer,
  TransferId,
  Identity,
  JsCrashReport,
  NetworkStatus,
  PlaybackCommand,
  PlaybackInfo,
  PlaybackId,
  RecordingExport,
  RecordingId,
  RecordingList,
  RecordingStatus,
  OpenOpts,
  PairedHost,
  PairFileSummary,
  PairOutcome,
  PairTarget,
  ProbeId,
  Quality,
  SessionActionKind,
  SessionCombo,
  SessionId,
  SessionSummary,
  SshId,
  SshKey,
  SshOpenOpts,
  SshServer,
  SyncValue,
  TailnetStatus,
  TrustedHostKey,
  VirtualDisplayOpts,
  SavedCredentialInfo,
  TailscaleApiStatus,
  TailscaleDevice,
} from './types';

export type Unsubscribe = () => void;

/** 与 rsp_core::DEFAULT_PORT 一致。 */
export const RSP_DEFAULT_PORT = 7400;

export class RsCoreError extends Error {
  constructor(
    public kind:
      | 'network'
      | 'http'
      | 'conflict'
      | 'unsupported'
      | 'cancelled'
      | 'internal',
    message: string,
    public httpStatus?: number,
    public hubCode?: string,
    public current?: SyncValue,
  ) {
    super(message);
    this.name = 'RsCoreError';
  }
}

export interface RsCoreApi {
  init(cfg: {
    dataDir: string;
    appVersion: string;
    platform: DevicePlatform;
    filesDir?: string;
  }): Promise<void>;
  events: { subscribe(cb: (e: CoreEvent) => void): Unsubscribe };
  identity: {
    ensure(deviceName: string): Promise<Identity>;
  };
  pairing: {
    parseQr(text: string): PairTarget | null;
    start(
      t:
        | PairTarget
        | { addr: string; port?: number; code: string }
        | { fileId: string },
    ): Promise<PairOutcome>;
    /**
     * 读配对文件；口令留在原生，按 fileId 缓存 10 分钟。不是配对文件 →
     * `RsCoreError('internal', 'not a pairing file')`；uri 不是 `file://` → `RsCoreError('unsupported')`。
     */
    loadFile(src: { text: string } | { uri: string }): Promise<PairFileSummary>;
    /** 也取消文件配对。 */
    cancel(): void;
  };
  hosts: {
    paired(): Promise<PairedHost[]>;
    forget(hostId: string): Promise<void>;
  };
  sessions: {
    prewarm(hostIds: string[]): void;
    open(hostId: string, opts: OpenOpts): Promise<SessionId>;
    close(id: SessionId): void;
    list(): SessionSummary[];
    setQuality(id: SessionId, q: Quality): void;
    /** 会话中开关远端声音；Host 不支持声音时静默忽略。 */
    setAudio(id: SessionId, enabled: boolean): void;
    selectDisplay(id: SessionId, displayId: number): void;
    /** 请 Host 新建虚拟显示器（M3）；返回 requestId，结果经 `session.vdisplay`。没有原生绑定时抛 RsCoreError('unsupported')。 */
    createVirtualDisplay(id: SessionId, opts: VirtualDisplayOpts): number;
    /** 移除本会话建的虚拟显示器；不是自己建的 Host 忽略。 */
    releaseVirtualDisplay(id: SessionId, displayId: number): void;
    requestControl(id: SessionId): void;
    action(id: SessionId, kind: SessionActionKind, enable?: boolean): void;
    key(id: SessionId, usage: number, down: boolean, mods: number): void;
    text(id: SessionId, text: string): void;
    combo(id: SessionId, combo: SessionCombo): void;
    clipboardSend(id: SessionId, text: string): void;
    background(id: SessionId, inBackground: boolean): void;
    /** 07 未定义，原生适配层 若未实现应抛 `RsCoreError('unsupported')`；页面遇到 `unsupported` 时隐藏对应入口。 */
    respondControl(
      id: SessionId,
      requesterDeviceId: string,
      allow: boolean,
    ): void;
  };
  diag: {
    probe(hostId: string): Promise<ProbeId>;
    /** 07 未定义，原生适配层 若未实现应抛 `RsCoreError('unsupported')`；页面遇到 `unsupported` 时隐藏对应入口。 */
    /** 不带 port 时原生使用 RSP_DEFAULT_PORT。 */
    probeAddress(addr: string, port?: number): Promise<ProbeId>;
  };
  logs: {
    export(opts: { mask: boolean }): Promise<string>;
    write(
      level: 'debug' | 'info' | 'warn' | 'error',
      tag: string,
      msg: string,
    ): void;
  };
  ssh: {
    // docs/dev/07 写的是 Server[]；这里返回 SSH 专用的 SshServer[]，文档待更新。
    servers(): Promise<SshServer[]>;
    open(serverId: string, opts?: SshOpenOpts): Promise<SshId>;
    close(id: SshId): void;
    keys(): Promise<SshKey[]>;
    createKey(kind: 'se-p256' | 'ed25519', name: string): Promise<SshKey>;
    installKey(serverId: string, keyId: string): Promise<void>;
    trustHostKey(serverId: string, fp: string): Promise<void>;
    /** 07 未定义，原生适配层 若未实现应抛 `RsCoreError('unsupported')`；页面遇到 `unsupported` 时隐藏对应入口。 */
    trustedHostKeys(): Promise<TrustedHostKey[]>;
    /** 07 未定义，原生适配层 若未实现应抛 `RsCoreError('unsupported')`；页面遇到 `unsupported` 时隐藏对应入口。 */
    forgetHostKey(serverId: string): Promise<void>;
  };
  files: {
    list(hostId: string, path: string | null): Promise<FileListing>;
    download(hostId: string, remotePath: string): Promise<TransferId>;
    upload(
      hostId: string,
      localPath: string,
      remoteDir: string | null,
    ): Promise<TransferId>;
  };
  sftp: {
    list(serverId: string, path: string | null): Promise<FileListing>;
    download(serverId: string, remotePath: string): Promise<TransferId>;
    upload(
      serverId: string,
      localPath: string,
      remoteDir: string | null,
    ): Promise<TransferId>;
  };
  transfers: {
    list(): FileTransfer[];
    pause(id: TransferId): void;
    resume(id: TransferId): Promise<void>;
    cancel(id: TransferId): void;
    remove(id: TransferId): void;
  };
  tailnet: {
    /** 本构建是否带 App 内 tailnet。绑定缺失（旧原生包）时 resolve(false)，不抛错。 */
    available(): Promise<boolean>;
    /** tailnet_status()。不可用时 state='unavailable'。 */
    status(): Promise<TailnetStatus>;
    /** 节点已启动且 needsLogin 时重新发起交互登录；新的 authUrl 经 tailnet.state 到达。 */
    login(): Promise<void>;
    /** 登出 App 内节点；之后状态为 needsLogin。 */
    logout(): Promise<void>;
  };
  network: {
    /** 07 未定义，原生适配层 若未实现应抛 `RsCoreError('unsupported')`；页面遇到 `unsupported` 时隐藏对应入口。 */
    status(): Promise<NetworkStatus>;
  };
  settings: {
    /** 07 未定义，原生适配层 若未实现应抛 `RsCoreError('unsupported')`；页面遇到 `unsupported` 时隐藏对应入口。 */
    set(key: string, value: unknown): void;
  };
  /** 可选：FakeCore 不实现；createRsCore 总会提供，旧原生包缺绑定时调用抛 RsCoreError('unsupported')。 */
  crash?: {
    /** 同步落盘；false 表示 Rust 拒收。只传致命错误。 */
    recordJs(report: JsCrashReport): boolean;
  };
  /** 会话录制。可选：FakeCore 不实现；createRsCore 总会提供，旧原生包缺绑定时调用抛 RsCoreError('unsupported')。 */
  recording?: {
    start(sessionId: SessionId): Promise<RecordingStatus>;
    stop(sessionId: SessionId): void;
    status(sessionId: SessionId): RecordingStatus;
    list(): Promise<RecordingList>;
    remove(recordingId: RecordingId): Promise<void>;
    open(recordingId: RecordingId): Promise<PlaybackInfo>;
    control(playbackId: PlaybackId, command: PlaybackCommand): void;
    close(playbackId: PlaybackId): void;
    exportMp4(recordingId: RecordingId): Promise<RecordingExport>;
    discardExport(path: string): void;
  };
  adapters?: {
    available(): boolean;
    open(target: AdapterTarget, opts: AdapterOpenOpts): Promise<SessionId>;
    close(id: SessionId): void;
    list(): AdapterSummary[];
    submitCredentials(id: SessionId, creds: AdapterCredentials): void;
    cancelCredentials(id: SessionId): void;
    saved(target: AdapterTarget): SavedCredentialInfo;
    forget(target: AdapterTarget): void;
    canSave(): boolean;
    typePassword(id: SessionId): void;
    sendCtrlAltDel(id: SessionId): void;
    trustCert(id: SessionId, accept: boolean): void;
    resize(id: SessionId, viewWidth: number, viewHeight: number, scale: number): void;
    selectScreen(id: SessionId, index: number): void;
    /** RDP：会话里改显示器数量（1–4）；VNC 忽略。 */
    setMonitors(id: SessionId, count: number): void;
  };
  /** V10。旧原生包没有这组绑定时为 undefined；页面据此隐藏扫描、退回旧探测。 */
  discover?: {
    probe(
      addr: string,
      port: number,
      kind: DiscoverKind,
      timeoutMs: number,
    ): Promise<DiscoverProbe>;
    /** 返回 scanId；结果经 discover.found / discover.done 事件到达。 */
    scanLan(ports: number[]): Promise<string>;
    cancelScan(scanId: string): void;
  };
  /**
   * 10-24 Tailscale API 只读令牌。旧原生包没有这组绑定时为 undefined，页面隐藏入口。
   * 令牌只经 setToken 进原生钥匙串，任何方法都不返回令牌。
   */
  tailscaleApi?: {
    /**
     * 先用令牌请求一次设备列表，成功才保存；401/403 → RsCoreError('http', status)，不保存。
     * controlUrl 为 null 或空串走官方 Tailscale；否则是 Headscale 控制服务器（只接受
     * https://主机[:端口]，可带末尾的 /），令牌是它的 API key；地址不合法 →
     * RsCoreError('internal', 'bad control url')，不发请求。
     */
    setToken(
      token: string,
      controlUrl: string | null,
    ): Promise<TailscaleDevice[]>;
    clearToken(): Promise<void>;
    status(): Promise<TailscaleApiStatus>;
    /** 缓存 30 s，force 跳过缓存。没配置令牌 → RsCoreError('unsupported', 'tailscale api not configured')。 */
    devices(force: boolean): Promise<TailscaleDevice[]>;
  };
}

export const GAP_METHODS = [
  'files.list',
  'files.download',
  'files.upload',
  'sftp.list',
  'sftp.download',
  'sftp.upload',
  'transfers.list',
  'transfers.pause',
  'transfers.resume',
  'transfers.cancel',
  'transfers.remove',
  'sessions.respondControl',
  'diag.probeAddress',
  'ssh.trustedHostKeys',
  'ssh.forgetHostKey',
  'network.status',
  'settings.set',
] as const;
