/**
 * Rust exports use snake_case (for example, sessions_select_display).
 * UniFFI maps u64/i64 to bigint, u8/u16/u32 to number, and Option<T> to
 * T | undefined. Objects cross as JSON strings with fields matching types.ts;
 * IDs inside JSON objects are decimal strings. Generated async functions may
 * also accept an optional asyncOpts_ argument, which this adapter does not use.
 */
export interface EventSinkLike {
  onEvent(seq: bigint, payload: string): void;
}

export type CoreErrorTag =
  | 'Network'
  | 'Http'
  | 'Conflict'
  | 'Unsupported'
  | 'Cancelled'
  | 'Internal';

export type CoreErrorLike = {
  tag: CoreErrorTag;
  inner: {
    message: string;
    status?: number;
    hubCode?: string;
    currentJson?: string;
  };
};

export interface RsMobileBindings {
  discoverProbe?(
    addr: string,
    port: number,
    kind: string,
    timeoutMs: number,
  ): Promise<string>;
  discoverScanLan?(ports: number[]): Promise<bigint>;
  discoverScanCancel?(scanId: bigint): void;
  // controlUrl 为 undefined 走官方 Tailscale。
  tailscaleApiSetToken?(
    token: string,
    controlUrl: string | undefined,
  ): Promise<string>;
  tailscaleApiClearToken?(): Promise<void>;
  tailscaleApiStatus?(): Promise<string>;
  tailscaleApiDevices?(force: boolean): Promise<string>;
  coreInit(cfgJson: string): Promise<void>;
  coreShutdown(): void;
  eventsSubscribe(sink: EventSinkLike): bigint;
  eventsUnsubscribe(id: bigint): void;
  identityEnsure(deviceName: string): Promise<string>;
  pairingParseQr(text: string): string | undefined;
  pairingStart(argJson: string): Promise<string>;
  pairingLoadFile(srcJson: string): Promise<string>;
  pairingCancel(): void;
  hostsPaired(): Promise<string>;
  hostsForget(hostId: string): Promise<void>;
  sessionsPrewarm(hostIds: string[]): void;
  sessionsOpen(hostId: string, optsJson: string): Promise<bigint>;
  sessionsClose(id: bigint): void;
  sessionsList(): string;
  sessionsSetQuality(id: bigint, quality: string): void;
  sessionsSetAudio?(id: bigint, enabled: boolean): void;
  sessionsSelectDisplay(id: bigint, displayId: number): void;
  sessionsCreateVirtualDisplay?(id: bigint, optsJson: string): number;
  sessionsReleaseVirtualDisplay?(id: bigint, displayId: number): void;
  sessionsRequestControl(id: bigint): void;
  sessionsRespondControl(
    id: bigint,
    requesterDeviceId: string,
    allow: boolean,
  ): void;
  sessionsAction(id: bigint, kind: string, enable: boolean | undefined): void;
  sessionsKey(id: bigint, usage: number, down: boolean, mods: number): void;
  sessionsText(id: bigint, text: string): void;
  sessionsCombo(id: bigint, combo: string): void;
  sessionsClipboardSend(id: bigint, text: string): void;
  sessionsBackground(id: bigint, inBackground: boolean): void;
  diagProbe(hostId: string): Promise<bigint>;
  diagProbeAddress(addr: string, port: number | undefined): Promise<bigint>;
  logsExport(mask: boolean): Promise<string>;
  logsWrite(level: string, tag: string, msg: string): void;
  sshServers(): Promise<string>;
  sshOpen(serverId: string, optsJson: string | undefined): Promise<bigint>;
  sshClose(id: bigint): void;
  sshKeys(): Promise<string>;
  sshCreateKey(kind: string, name: string): Promise<string>;
  sshInstallKey(serverId: string, keyId: string): Promise<void>;
  sshTrustHostKey(serverId: string, fp: string): Promise<void>;
  sshTrustedHostKeys(): Promise<string>;
  sshForgetHostKey(serverId: string): Promise<void>;
  filesList?(hostId: string, path: string | undefined): Promise<string>;
  filesDownload?(hostId: string, remotePath: string): Promise<string>;
  filesUpload?(
    hostId: string,
    localPath: string,
    remoteDir: string | undefined,
  ): Promise<string>;
  sftpList?(serverId: string, path: string | undefined): Promise<string>;
  sftpDownload?(serverId: string, remotePath: string): Promise<string>;
  sftpUpload?(
    serverId: string,
    localPath: string,
    remoteDir: string | undefined,
  ): Promise<string>;
  transfersList?(): string;
  transfersPause?(id: string): void;
  transfersResume?(id: string): Promise<void>;
  transfersCancel?(id: string): void;
  transfersRemove?(id: string): void;
  tailnetAvailable?(): Promise<boolean>;
  tailnetStatus?(): Promise<string>;
  tailnetLogin?(): Promise<void>;
  tailnetLogout?(): Promise<void>;
  networkStatus(): Promise<string>;
  settingsSet(key: string, valueJson: string): void;
  crashRecordJs?(reportJson: string): boolean;
  recordingStart?(sessionId: bigint): Promise<string>;
  recordingStop?(sessionId: bigint): void;
  recordingStatus?(sessionId: bigint): string;
  recordingsList?(): Promise<string>;
  recordingsDelete?(recordingId: string): Promise<void>;
  playbackOpen?(recordingId: string): Promise<string>;
  playbackControl?(playbackId: bigint, commandJson: string): void;
  playbackClose?(playbackId: bigint): void;
  recordingsExport?(recordingId: string): Promise<string>;
  recordingsDiscardExport?(path: string): void;
  adaptersOpen?(targetJson: string, optsJson: string): Promise<bigint>;
  adaptersClose?(id: bigint): void;
  adaptersList?(): string;
  adaptersSubmitCredentials?(
    id: bigint,
    user: string | undefined,
    password: string,
    domain: string | undefined,
    save: boolean,
  ): void;
  adaptersCancelCredentials?(id: bigint): void;
  adaptersSaved?(targetJson: string): string;
  adaptersForget?(targetJson: string): void;
  adaptersCanSave?(): boolean;
  adaptersTypePassword?(id: bigint): void;
  adaptersSendCtrlAltDel?(id: bigint): void;
  adaptersTrustCert?(id: bigint, accept: boolean): void;
  adaptersResize?(
    id: bigint,
    viewWidth: number,
    viewHeight: number,
    scale: number,
  ): void;
  adaptersSelectScreen?(id: bigint, index: number): void;
  adaptersSetMonitors?(id: bigint, count: number): void;
}
