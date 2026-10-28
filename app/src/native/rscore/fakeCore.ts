import type { RsCoreApi } from './RsCoreApi';
import { RSP_DEFAULT_PORT, RsCoreError } from './RsCoreApi';
import {
  FIXTURE_NOW,
  SELF_DEVICE_ID,
  directory as fixtureDirectory,
  fileTrees as fixtureFileTrees,
  liveSession,
  netTailnetOk,
  pairFileSummary,
  pairFileText,
  pairedHosts as fixturePairedHosts,
  sshKeys as fixtureSshKeys,
  sshServers as fixtureSshServers,
  tailnetUnavailable,
  trustedHostKeys as fixtureTrustedHostKeys,
} from './fixtures';
import type {
  CoreEvent,
  DevicePlatform,
  Directory,
  FileEntry,
  FileListing,
  FileSourceRef,
  FileTransfer,
  FileTransferFail,
  Identity,
  NetworkStatus,
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
  TailnetRouteStatus,
  TailnetStatus,
  TailscaleApiStatus,
  TailscaleDevice,
  TrustedHostKey,
  VirtualDisplayOpts,
} from './types';

export type FakeFixtures = {
  directory: Directory;
  pairFiles: Record<string, PairFileSummary>;
  pairedHosts: PairedHost[];
  sshServers: SshServer[];
  sshKeys: SshKey[];
  trustedHostKeys: TrustedHostKey[];
  sessions: SessionSummary[];
  tailnetAvailable: boolean;
  tailnet: TailnetStatus;
  network: NetworkStatus;
  identity: Identity | null;
  fileTrees: Record<string, Record<string, FileEntry[]>>;
};

type Call = { method: string; args: unknown[] };
type Listener = (event: CoreEvent) => void;

const CONTROL_URL = /^https:\/\/([\w.-]+|\[[\d:a-f.]+\])(:\d+)?\/?$/i;

/** Rust 的 parse_control_url 的简化版：null / 空白 → null，只收 https://主机[:端口][/]。 */
function fakeControlUrl(raw: string | null): string | null {
  const value = raw?.trim() ?? '';
  if (value === '') {
    return null;
  }
  const match = CONTROL_URL.exec(value);
  if (match === null) {
    throw new RsCoreError('internal', 'bad control url');
  }
  const [, host = '', port = ''] = match;
  return `https://${host.toLowerCase()}${port === ':443' ? '' : port}`;
}

function copy<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map(item => copy(item)) as T;
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, copy(item)]),
    ) as T;
  }
  return value;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(item => typeof item === 'string');
}

function isNetworkEntry(value: unknown): boolean {
  if (!isPlainObject(value) || Object.keys(value).length !== 4) {
    return false;
  }
  return (
    typeof value.id === 'string' &&
    typeof value.name === 'string' &&
    isStringArray(value.cidrs) &&
    isStringArray(value.vpnPools)
  );
}

function defaultFixtures(): FakeFixtures {
  return {
    directory: copy(fixtureDirectory),
    pairFiles: { [pairFileText]: copy(pairFileSummary) },
    pairedHosts: copy(fixturePairedHosts),
    sshServers: copy(fixtureSshServers),
    sshKeys: copy(fixtureSshKeys),
    trustedHostKeys: copy(fixtureTrustedHostKeys),
    sessions: [copy(liveSession)],
    tailnetAvailable: false,
    tailnet: copy(tailnetUnavailable),
    network: copy(netTailnetOk),
    identity: null,
    fileTrees: copy(fixtureFileTrees),
  };
}

const TRANSFER_STEPS = 10;
const DEFAULT_TICK_MS = 100;

function sourceKey(source: FileSourceRef): string {
  return source.kind === 'computer'
    ? `computer:${source.hostId}`
    : `server:${source.serverId}`;
}

function sourcePath(source: FileSourceRef): string {
  return source.kind === 'computer' ? 'files' : 'sftp';
}

function lastSegment(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function parentPath(path: string): string | null {
  const parts = path.split('/').filter(Boolean);
  if (parts.length <= 1) {
    return null;
  }
  return `/${parts.slice(0, -1).join('/')}`;
}

function findFile(
  tree: Record<string, FileEntry[]> | undefined,
  remotePath: string,
): FileEntry | undefined {
  if (tree === undefined) {
    return undefined;
  }
  const parent = parentPath(remotePath);
  if (parent === null) {
    return undefined;
  }
  return tree[parent]?.find(
    entry => entry.name === lastSegment(remotePath) && entry.kind !== 'dir',
  );
}

function parsePairingQr(text: string): PairTarget | null {
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }

  if (url.protocol !== 'rsp:' || url.hostname !== 'pair') {
    return null;
  }

  const params = url.searchParams;
  const version = params.get('v');
  const hostId = params.get('h');
  const fp = params.get('fp');
  const code = params.get('c');
  const name = params.get('n');
  const addressList = params.get('a');
  if (
    version !== '1' ||
    hostId === null ||
    fp === null ||
    code === null ||
    !/^[0-9]{6}$/.test(code) ||
    name === null ||
    addressList === null
  ) {
    return null;
  }

  return {
    v: 1,
    hostId,
    fp,
    code,
    name,
    addrs: addressList.split(','),
  };
}

export class FakeCore implements RsCoreApi {
  readonly calls: Call[] = [];
  readonly transfersById = new Map<string, FileTransfer>();

  private fixtures: FakeFixtures;
  private readonly failures = new Map<string, Error>();
  private readonly listeners = new Set<Listener>();
  private pendingPairing: ((outcome: PairOutcome) => void) | null = null;
  private sessionSequence = 0;
  private vdisplayRequestId = 0;
  private probeSequence = 0;
  private probeAddressSequence = 0;
  private discoverScanSequence = 0;
  private tailscaleConfigured = false;
  private tailscaleDevices: TailscaleDevice[] = [];
  private tailscaleStatus: TailscaleApiStatus = {
    configured: false,
    controlUrl: null,
    lastError: null,
    lastOkAt: null,
    deviceCount: null,
  };
  private sshSequence = 0;
  private keySequence = 0;
  private transferSequence = 0;
  private readonly transferTimers = new Map<
    string,
    ReturnType<typeof setTimeout>
  >();
  private readonly tickMs: number;

  constructor(
    fixtures: Partial<FakeFixtures> = {},
    opts: { tickMs?: number } | number = {},
  ) {
    this.tickMs =
      (typeof opts === 'number' ? opts : opts.tickMs) ?? DEFAULT_TICK_MS;
    this.fixtures = defaultFixtures();
    this.setFixtures(fixtures);
  }

  setFixtures(fixtures: Partial<FakeFixtures>): void {
    const next = { ...this.fixtures };
    for (const key of Object.keys(fixtures) as (keyof FakeFixtures)[]) {
      const value = fixtures[key];
      if (value !== undefined) {
        (next as Record<keyof FakeFixtures, unknown>)[key] = copy(value);
      }
    }
    next.directory.others ??= [];
    this.fixtures = next;
  }

  setTailnet(next: {
    available?: boolean;
    status?: TailnetStatus;
    route?: TailnetRouteStatus | null;
  }): void {
    if (next.available !== undefined) {
      this.fixtures.tailnetAvailable = next.available;
    }
    if (next.status !== undefined) {
      const status = copy(next.status);
      this.fixtures.tailnet = status;
      this.emit({
        type: 'tailnet.state',
        state: status.state,
        authUrl: status.authUrl,
        ips: copy(status.self?.ips ?? []),
        derpRegion: status.self?.derpRegion ?? '',
        message: null,
      });
    }
    if (next.route !== undefined) {
      if (next.route === null) {
        delete this.fixtures.network.tailnet;
      } else {
        this.fixtures.network.tailnet = copy(next.route);
      }
      this.emit({
        type: 'network.changed',
        status: copy(this.fixtures.network),
      });
    }
  }

  /**
   * Tailscale API 令牌：devices 是令牌有效时返回的列表；configured 模拟钥匙串里已有令牌，
   * controlUrl 模拟和它一起存的 Headscale 控制服务器。
   */
  setTailscale(next: {
    configured?: boolean;
    devices?: TailscaleDevice[];
    controlUrl?: string | null;
  }): void {
    if (next.configured !== undefined) {
      this.tailscaleConfigured = next.configured;
      this.tailscaleStatus = {
        ...this.tailscaleStatus,
        configured: next.configured,
      };
    }
    if (next.controlUrl !== undefined) {
      this.tailscaleStatus = {
        ...this.tailscaleStatus,
        controlUrl: next.controlUrl,
      };
    }
    if (next.devices !== undefined) {
      this.tailscaleDevices = copy(next.devices);
    }
  }

  failNext(method: string, error: Error): void {
    this.failures.set(method, error);
  }

  failTransfer(id: string, fail: FileTransferFail): void {
    const transfer = this.requireTransfer(id);
    this.stopTimer(id);
    transfer.state = 'failed';
    transfer.fail = fail;
    transfer.bytesPerSec = 0;
    transfer.updatedAt = FIXTURE_NOW;
    this.emitTransfer(transfer);
  }

  interruptTransfer(id: string): void {
    const transfer = this.requireTransfer(id);
    this.stopTimer(id);
    transfer.state = 'interrupted';
    transfer.bytesPerSec = 0;
    transfer.updatedAt = FIXTURE_NOW;
    this.emitTransfer(transfer);
  }

  emit(event: CoreEvent): void {
    for (const listener of [...this.listeners]) {
      listener(event);
    }
  }

  resolvePairing(outcome: PairOutcome): void {
    const resolve = this.pendingPairing;
    if (resolve === null) {
      return;
    }
    this.pendingPairing = null;
    resolve(copy(outcome));
  }

  subscriberCount(): number {
    return this.listeners.size;
  }

  private record(method: string, ...args: unknown[]): void {
    this.calls.push({ method, args });
    const failure = this.failures.get(method);
    if (failure !== undefined) {
      this.failures.delete(method);
      throw failure;
    }
  }

  private nextSessionId(): SessionId {
    this.sessionSequence += 1;
    return `s${this.sessionSequence}`;
  }

  private requireTransfer(id: string): FileTransfer {
    const transfer = this.transfersById.get(id);
    if (transfer === undefined) {
      throw new RsCoreError('http', `transfers.notFound: ${id}`, 404);
    }
    return transfer;
  }

  private emitTransfer(transfer: FileTransfer): void {
    this.emit({ type: 'file.transfer', transfer: copy(transfer) });
  }

  private stopTimer(id: string): void {
    const timer = this.transferTimers.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.transferTimers.delete(id);
    }
  }

  private finishVerifying(transfer: FileTransfer): void {
    if (transfer.state !== 'verifying') {
      return;
    }
    transfer.state = 'completed';
    transfer.bytesPerSec = 0;
    transfer.updatedAt = FIXTURE_NOW;
    if (transfer.direction === 'download') {
      transfer.localPath = `/fake/files/${transfer.name}`;
    } else {
      transfer.savedName = transfer.name;
    }
    this.emitTransfer(transfer);
  }

  private scheduleTransfer(transfer: FileTransfer): void {
    const id = transfer.transferId;
    this.stopTimer(id);
    if (
      transfer.state !== 'running' &&
      transfer.state !== 'queued' &&
      transfer.state !== 'verifying'
    ) {
      return;
    }
    const delay =
      transfer.state === 'queued' || transfer.state === 'verifying'
        ? 0
        : this.tickMs;
    const timer = setTimeout(() => {
      this.transferTimers.delete(id);
      if (transfer.state === 'queued') {
        transfer.state = 'running';
        transfer.updatedAt = FIXTURE_NOW;
        this.emitTransfer(transfer);
        this.scheduleTransfer(transfer);
        return;
      }
      if (transfer.state === 'verifying') {
        this.finishVerifying(transfer);
        return;
      }
      if (transfer.state !== 'running') {
        return;
      }
      const step = Math.max(1, Math.ceil(transfer.size / TRANSFER_STEPS));
      transfer.done = Math.min(transfer.size, transfer.done + step);
      transfer.bytesPerSec = Math.round((step * 1_000) / this.tickMs);
      transfer.updatedAt = FIXTURE_NOW;
      if (transfer.done >= transfer.size) {
        transfer.done = transfer.size;
        transfer.state = 'verifying';
        transfer.bytesPerSec = 0;
      }
      this.emitTransfer(transfer);
      this.scheduleTransfer(transfer);
    }, delay);
    this.transferTimers.set(id, timer);
  }

  private listFiles(source: FileSourceRef, path: string | null): FileListing {
    let resolvedPath = path;
    if (source.kind === 'computer') {
      if (
        !this.fixtures.pairedHosts.some(host => host.hostId === source.hostId)
      ) {
        throw new RsCoreError(
          'unsupported',
          `files.unsupported: ${source.hostId}`,
        );
      }
      resolvedPath ??= '/Users/demo';
    } else {
      if (
        !this.fixtures.sshServers.some(
          server => server.serverId === source.serverId,
        )
      ) {
        throw new RsCoreError(
          'http',
          `sftp.serverNotFound: ${source.serverId}`,
          404,
        );
      }
      resolvedPath ??= '/home/demo';
    }
    const tree = this.fixtures.fileTrees[sourceKey(source)];
    const entries = tree?.[resolvedPath];
    if (tree === undefined || entries === undefined) {
      throw new RsCoreError(
        'http',
        `${sourcePath(source)}.notFound: ${resolvedPath}`,
        404,
      );
    }
    return {
      path: resolvedPath,
      parent:
        resolvedPath ===
        (source.kind === 'computer' ? '/Users/demo' : '/home/demo')
          ? null
          : parentPath(resolvedPath),
      entries: copy(entries),
      truncated: false,
      netPath: source.kind === 'computer' ? 'tailnetDirect' : null,
    };
  }

  private startTransfer(
    source: FileSourceRef,
    direction: 'upload' | 'download',
    name: string,
    remotePath: string | null,
    size: number,
  ): string {
    this.transferSequence += 1;
    const id = `fake-t${this.transferSequence}`;
    const transfer: FileTransfer = {
      transferId: id,
      direction,
      source: copy(source),
      name,
      remotePath,
      localPath: null,
      size,
      done: 0,
      bytesPerSec: 0,
      state: 'queued',
      fail: null,
      netPath: source.kind === 'computer' ? 'tailnetDirect' : null,
      savedName: null,
      startedAt: FIXTURE_NOW,
      updatedAt: FIXTURE_NOW,
    };
    this.transfersById.set(id, transfer);
    this.emitTransfer(transfer);
    this.scheduleTransfer(transfer);
    return id;
  }

  private requireFileSource(source: FileSourceRef, remotePath: string): number {
    const tree = this.fixtures.fileTrees[sourceKey(source)];
    const file = findFile(tree, remotePath);
    if (source.kind === 'computer') {
      if (
        !this.fixtures.pairedHosts.some(host => host.hostId === source.hostId)
      ) {
        throw new RsCoreError(
          'unsupported',
          `files.unsupported: ${source.hostId}`,
        );
      }
      if (tree === undefined || file === undefined) {
        throw new RsCoreError('http', `files.notFound: ${remotePath}`, 404);
      }
      return file.size ?? 0;
    }
    if (
      !this.fixtures.sshServers.some(
        server => server.serverId === source.serverId,
      )
    ) {
      throw new RsCoreError(
        'http',
        `sftp.serverNotFound: ${source.serverId}`,
        404,
      );
    }
    if (tree === undefined || file === undefined) {
      throw new RsCoreError('http', `sftp.notFound: ${remotePath}`, 404);
    }
    return file.size ?? 0;
  }

  async init(cfg: {
    dataDir: string;
    appVersion: string;
    platform: DevicePlatform;
    filesDir?: string;
  }): Promise<void> {
    this.record('init', cfg);
  }

  events = {
    subscribe: (cb: Listener) => {
      this.record('events.subscribe', cb);
      this.listeners.add(cb);
      return () => {
        this.listeners.delete(cb);
      };
    },
  };

  identity = {
    ensure: async (deviceName: string): Promise<Identity> => {
      this.record('identity.ensure', deviceName);
      if (this.fixtures.identity === null) {
        this.fixtures.identity = {
          deviceId: SELF_DEVICE_ID,
          fp: 'fake-fp',
        };
      }
      return copy(this.fixtures.identity);
    },
  };

  pairing = {
    parseQr: (text: string): PairTarget | null => {
      this.record('pairing.parseQr', text);
      return parsePairingQr(text);
    },
    start: async (
      target:
        | PairTarget
        | { addr: string; port?: number; code: string }
        | { fileId: string },
    ): Promise<PairOutcome> => {
      this.record('pairing.start', target);
      return new Promise<PairOutcome>(resolve => {
        this.pendingPairing = resolve;
      });
    },
    loadFile: async (
      src: { text: string } | { uri: string },
    ): Promise<PairFileSummary> => {
      this.record('pairing.loadFile', 'text' in src ? { text: true } : src);
      let key: string | undefined;
      if ('text' in src) {
        key = src.text.trim();
        const exact = this.fixtures.pairFiles[key];
        if (exact !== undefined) {
          return copy(exact);
        }
        key = [...Object.keys(this.fixtures.pairFiles)].find(
          candidate => candidate.trim() === key,
        );
      } else {
        if (!src.uri.startsWith('file://')) {
          throw new RsCoreError('unsupported', 'unsupported pairing file uri');
        }
        key = src.uri;
      }
      const summary =
        key === undefined ? undefined : this.fixtures.pairFiles[key];
      if (summary === undefined) {
        throw new RsCoreError('internal', 'not a pairing file');
      }
      return copy(summary);
    },
    cancel: (): void => {
      this.record('pairing.cancel');
      const resolve = this.pendingPairing;
      this.pendingPairing = null;
      if (resolve !== null) {
        resolve({ status: 'cancelled' });
      }
    },
  };

  hosts = {
    paired: async (): Promise<PairedHost[]> => {
      this.record('hosts.paired');
      return copy(this.fixtures.pairedHosts);
    },
    forget: async (hostId: string): Promise<void> => {
      this.record('hosts.forget', hostId);
      this.fixtures.pairedHosts = this.fixtures.pairedHosts.filter(
        host => host.hostId !== hostId,
      );
    },
  };

  sessions = {
    prewarm: (hostIds: string[]): void => {
      this.record('sessions.prewarm', hostIds);
    },
    open: async (hostId: string, opts: OpenOpts): Promise<SessionId> => {
      this.record('sessions.open', hostId, opts);
      const id = this.nextSessionId();
      const computer = this.fixtures.directory.computers.find(
        candidate => candidate.hostId === hostId,
      );
      this.fixtures.sessions.push({
        id,
        hostId,
        hostName: computer?.name ?? hostId,
        phase: 'connecting',
        role: opts.intent === 'control' ? 'controller' : 'viewer',
        path: null,
        transport: null,
        startedAt: FIXTURE_NOW,
        displayId: opts.displayId ?? null,
      });
      return id;
    },
    close: (id: SessionId): void => {
      this.record('sessions.close', id);
      const session = this.fixtures.sessions.find(
        candidate => candidate.id === id,
      );
      if (session === undefined) {
        return;
      }
      this.fixtures.sessions = this.fixtures.sessions.filter(
        candidate => candidate.id !== id,
      );
      this.emit({
        type: 'session.error',
        sessionId: id,
        hostId: session.hostId,
        reason: 'userClosed',
      });
    },
    list: (): SessionSummary[] => {
      this.record('sessions.list');
      return [...this.fixtures.sessions];
    },
    setQuality: (id: SessionId, quality: Quality): void => {
      this.record('sessions.setQuality', id, quality);
    },
    setAudio: (id: SessionId, enabled: boolean): void => {
      this.record('sessions.setAudio', id, enabled);
    },
    selectDisplay: (id: SessionId, displayId: number): void => {
      this.record('sessions.selectDisplay', id, displayId);
    },
    createVirtualDisplay: (id: SessionId, opts: VirtualDisplayOpts): number => {
      this.record('sessions.createVirtualDisplay', id, opts);
      return ++this.vdisplayRequestId;
    },
    releaseVirtualDisplay: (id: SessionId, displayId: number): void => {
      this.record('sessions.releaseVirtualDisplay', id, displayId);
    },
    requestControl: (id: SessionId): void => {
      this.record('sessions.requestControl', id);
    },
    action: (
      id: SessionId,
      kind: SessionActionKind,
      enable?: boolean,
    ): void => {
      this.record('sessions.action', id, kind, enable);
    },
    key: (id: SessionId, usage: number, down: boolean, mods: number): void => {
      this.record('sessions.key', id, usage, down, mods);
    },
    text: (id: SessionId, text: string): void => {
      this.record('sessions.text', id, text);
    },
    combo: (id: SessionId, combo: SessionCombo): void => {
      this.record('sessions.combo', id, combo);
    },
    clipboardSend: (id: SessionId, text: string): void => {
      this.record('sessions.clipboardSend', id, text);
    },
    background: (id: SessionId, inBackground: boolean): void => {
      this.record('sessions.background', id, inBackground);
    },
    respondControl: (
      id: SessionId,
      requesterDeviceId: string,
      allow: boolean,
    ): void => {
      this.record('sessions.respondControl', id, requesterDeviceId, allow);
    },
  };

  diag = {
    probe: async (hostId: string): Promise<ProbeId> => {
      this.record('diag.probe', hostId);
      this.probeSequence += 1;
      return `p${this.probeSequence}`;
    },
    probeAddress: async (addr: string, port?: number): Promise<ProbeId> => {
      const effective = port ?? RSP_DEFAULT_PORT;
      this.record('diag.probeAddress', addr, effective);
      if (!Number.isInteger(effective) || effective < 1 || effective > 65535) {
        throw new RsCoreError('internal', 'bad port');
      }
      this.probeAddressSequence += 1;
      return `p${this.probeAddressSequence}`;
    },
  };

  discover: RsCoreApi['discover'] = {
    probe: async (addr, port, kind, timeoutMs) => {
      this.record('discover.probe', addr, port, kind, timeoutMs);
      return { addr, port, outcome: 'timeout', ms: timeoutMs };
    },
    scanLan: async ports => {
      this.record('discover.scanLan', ports);
      this.discoverScanSequence += 1;
      return String(this.discoverScanSequence);
    },
    cancelScan: scanId => {
      this.record('discover.cancelScan', scanId);
    },
  };

  tailscaleApi: RsCoreApi['tailscaleApi'] = {
    setToken: async (token, controlUrl) => {
      this.record('tailscaleApi.setToken', token, controlUrl);
      const control = fakeControlUrl(controlUrl);
      if (token.trim() === '') {
        throw new RsCoreError('internal', 'empty token');
      }
      this.tailscaleConfigured = true;
      this.tailscaleStatus = {
        configured: true,
        controlUrl: control,
        lastError: null,
        lastOkAt: FIXTURE_NOW,
        deviceCount: this.tailscaleDevices.length,
      };
      return copy(this.tailscaleDevices);
    },
    clearToken: async () => {
      this.record('tailscaleApi.clearToken');
      this.tailscaleConfigured = false;
      this.tailscaleStatus = {
        configured: false,
        controlUrl: null,
        lastError: null,
        lastOkAt: null,
        deviceCount: null,
      };
    },
    status: async () => {
      this.record('tailscaleApi.status');
      return { ...this.tailscaleStatus };
    },
    devices: async force => {
      this.record('tailscaleApi.devices', force);
      if (!this.tailscaleConfigured) {
        throw new RsCoreError('unsupported', 'tailscale api not configured');
      }
      this.tailscaleStatus = {
        ...this.tailscaleStatus,
        configured: true,
        lastError: null,
        lastOkAt: FIXTURE_NOW,
        deviceCount: this.tailscaleDevices.length,
      };
      return copy(this.tailscaleDevices);
    },
  };

  logs = {
    export: async (opts: { mask: boolean }): Promise<string> => {
      this.record('logs.export', opts);
      return opts.mask ? '/tmp/rs-logs-masked.zip' : '/tmp/rs-logs.zip';
    },
    write: (
      level: 'debug' | 'info' | 'warn' | 'error',
      tag: string,
      msg: string,
    ): void => {
      this.record('logs.write', level, tag, msg);
    },
  };

  ssh = {
    servers: async (): Promise<SshServer[]> => {
      this.record('ssh.servers');
      return copy(this.fixtures.sshServers);
    },
    open: async (serverId: string, opts?: SshOpenOpts): Promise<SshId> => {
      this.record('ssh.open', serverId, opts);
      this.sshSequence += 1;
      return `ssh${this.sshSequence}`;
    },
    close: (id: SshId): void => {
      this.record('ssh.close', id);
    },
    keys: async (): Promise<SshKey[]> => {
      this.record('ssh.keys');
      return copy(this.fixtures.sshKeys);
    },
    createKey: async (
      kind: 'se-p256' | 'ed25519',
      name: string,
    ): Promise<SshKey> => {
      this.record('ssh.createKey', kind, name);
      this.keySequence += 1;
      const key: SshKey = {
        keyId: `key-${this.keySequence}`,
        name,
        kind,
        fp: `fake-fp-${this.keySequence}`,
        publicKey: `${
          kind === 'se-p256' ? 'ecdsa-sha2-nistp256' : 'ssh-ed25519'
        } FAKE-${this.keySequence}`,
        createdAt: FIXTURE_NOW,
        installedOn: [],
      };
      this.fixtures.sshKeys.push(key);
      return copy(key);
    },
    installKey: async (
      serverId: string,
      keyId: string,
    ): Promise<void> => {
      this.record('ssh.installKey', serverId, keyId);
      const key = this.fixtures.sshKeys.find(
        candidate => candidate.keyId === keyId,
      );
      if (key !== undefined && !key.installedOn.includes(serverId)) {
        key.installedOn.push(serverId);
      }
    },
    trustHostKey: async (serverId: string, fp: string): Promise<void> => {
      this.record('ssh.trustHostKey', serverId, fp);
      if (
        !this.fixtures.trustedHostKeys.some(
          key => key.serverId === serverId && key.fp === fp,
        )
      ) {
        this.fixtures.trustedHostKeys.push({
          serverId,
          keyType: 'ssh-ed25519',
          fp,
          addedAt: FIXTURE_NOW,
        });
      }
    },
    trustedHostKeys: async (): Promise<TrustedHostKey[]> => {
      this.record('ssh.trustedHostKeys');
      return copy(this.fixtures.trustedHostKeys);
    },
    forgetHostKey: async (serverId: string): Promise<void> => {
      this.record('ssh.forgetHostKey', serverId);
      this.fixtures.trustedHostKeys = this.fixtures.trustedHostKeys.filter(
        key => key.serverId !== serverId,
      );
    },
  };

  files: RsCoreApi['files'] = {
    list: async (hostId, path) => {
      this.record('files.list', hostId, path);
      return this.listFiles({ kind: 'computer', hostId }, path);
    },
    download: async (hostId, remotePath) => {
      this.record('files.download', hostId, remotePath);
      const source: FileSourceRef = { kind: 'computer', hostId };
      const size = this.requireFileSource(source, remotePath);
      return this.startTransfer(
        source,
        'download',
        lastSegment(remotePath),
        remotePath,
        size,
      );
    },
    upload: async (hostId, localPath, remoteDir) => {
      this.record('files.upload', hostId, localPath, remoteDir);
      const source: FileSourceRef = { kind: 'computer', hostId };
      if (!this.fixtures.pairedHosts.some(host => host.hostId === hostId)) {
        throw new RsCoreError('unsupported', `files.unsupported: ${hostId}`);
      }
      const name = lastSegment(localPath);
      return this.startTransfer(
        source,
        'upload',
        name,
        remoteDir,
        Math.max(1, 4 * 1024 * 1024),
      );
    },
  };

  sftp: RsCoreApi['sftp'] = {
    list: async (serverId, path) => {
      this.record('sftp.list', serverId, path);
      return this.listFiles({ kind: 'server', serverId }, path);
    },
    download: async (serverId, remotePath) => {
      this.record('sftp.download', serverId, remotePath);
      const source: FileSourceRef = { kind: 'server', serverId };
      const size = this.requireFileSource(source, remotePath);
      return this.startTransfer(
        source,
        'download',
        lastSegment(remotePath),
        remotePath,
        size,
      );
    },
    upload: async (serverId, localPath, remoteDir) => {
      this.record('sftp.upload', serverId, localPath, remoteDir);
      if (
        !this.fixtures.sshServers.some(server => server.serverId === serverId)
      ) {
        throw new RsCoreError('http', `sftp.serverNotFound: ${serverId}`, 404);
      }
      return this.startTransfer(
        { kind: 'server', serverId },
        'upload',
        lastSegment(localPath),
        remoteDir,
        Math.max(1, 4 * 1024 * 1024),
      );
    },
  };

  transfers: RsCoreApi['transfers'] = {
    list: () => {
      this.record('transfers.list');
      return [...this.transfersById.values()].map(transfer => copy(transfer));
    },
    pause: id => {
      this.record('transfers.pause', id);
      const transfer = this.requireTransfer(id);
      if (transfer.state === 'paused') {
        return;
      }
      if (
        transfer.state !== 'queued' &&
        transfer.state !== 'running' &&
        transfer.state !== 'interrupted'
      ) {
        throw new RsCoreError('http', `transfers.notActive: ${id}`, 409);
      }
      this.stopTimer(id);
      transfer.state = 'paused';
      transfer.bytesPerSec = 0;
      transfer.updatedAt = FIXTURE_NOW;
      this.emitTransfer(transfer);
    },
    resume: async id => {
      this.record('transfers.resume', id);
      const transfer = this.requireTransfer(id);
      if (transfer.state === 'completed' || transfer.state === 'cancelled') {
        throw new RsCoreError('http', `transfers.notResumable: ${id}`, 409);
      }
      if (transfer.state === 'verifying') {
        throw new RsCoreError('http', `transfers.notResumable: ${id}`, 409);
      }
      if (transfer.state === 'running' || transfer.state === 'queued') {
        return;
      }
      transfer.state = 'running';
      transfer.fail = null;
      transfer.updatedAt = FIXTURE_NOW;
      this.emitTransfer(transfer);
      this.scheduleTransfer(transfer);
    },
    cancel: id => {
      this.record('transfers.cancel', id);
      const transfer = this.requireTransfer(id);
      if (
        transfer.state === 'completed' ||
        transfer.state === 'failed' ||
        transfer.state === 'cancelled'
      ) {
        return;
      }
      this.stopTimer(id);
      transfer.state = 'cancelled';
      transfer.fail = null;
      transfer.bytesPerSec = 0;
      transfer.updatedAt = FIXTURE_NOW;
      this.emitTransfer(transfer);
    },
    remove: id => {
      this.record('transfers.remove', id);
      this.requireTransfer(id);
      this.stopTimer(id);
      this.transfersById.delete(id);
      this.emit({ type: 'file.transferRemoved', transferId: id });
    },
  };

  tailnet = {
    available: async (): Promise<boolean> => {
      this.record('tailnet.available');
      return this.fixtures.tailnetAvailable;
    },
    status: async (): Promise<TailnetStatus> => {
      this.record('tailnet.status');
      return copy(this.fixtures.tailnet);
    },
    login: async (): Promise<void> => {
      this.record('tailnet.login');
      if (!this.fixtures.tailnetAvailable) {
        throw new RsCoreError('unsupported', 'tailnet.login');
      }
    },
    logout: async (): Promise<void> => {
      this.record('tailnet.logout');
      if (!this.fixtures.tailnetAvailable) {
        throw new RsCoreError('unsupported', 'tailnet.logout');
      }
    },
  };

  network = {
    status: async (): Promise<NetworkStatus> => {
      this.record('network.status');
      return copy(this.fixtures.network);
    },
  };

  settings = {
    set: (key: string, value: unknown): void => {
      this.record('settings.set', key, value);
      let valid = true;
      switch (key) {
        case 'quality.default':
          valid =
            value === 'auto' ||
            value === 'clear' ||
            value === 'smooth' ||
            value === 'eco';
          break;
        case 'tailnet.mode':
          valid =
            value === 'system' || value === 'embedded' || value === 'auto';
          break;
        case 'tailnet.controlUrl':
          valid =
            value === null ||
            (typeof value === 'string' && /^https?:\/\/[^\s/?#]+/.test(value));
          break;
        case 'device.appActive':
          valid = typeof value === 'boolean';
          break;
        case 'prewarm.enabled':
          valid = typeof value === 'boolean';
          break;
        case 'networks':
          valid =
            value === null ||
            (Array.isArray(value) && value.every(isNetworkEntry));
          break;
        default:
          break;
      }
      if (!valid) {
        throw new RsCoreError('internal', `bad value for ${key}`);
      }
    },
  };
}

export function createFakeCore(
  fixtures?: Partial<FakeFixtures>,
  opts?: { tickMs?: number } | number,
): FakeCore {
  return new FakeCore(fixtures, opts);
}
