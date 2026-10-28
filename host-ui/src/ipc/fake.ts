import type { ApprovalKind, ApprovalRequest, EventName, IpcEvents } from './events';
import {
  buildFixture,
  HOST_FP_B64,
  HOST_ID,
  HOST_NAME,
  PAIRING_CODES,
  type FakeState,
} from './fixtures';
import type { RpcErrorShape, Transport } from './transport';
import { AGENT_UNAVAILABLE } from './transport';
import type {
  ApprovalPendingResult,
  ApprovalRespondParams,
  HostSettings,
  MethodName,
  PairFileInfo,
  PairFileState,
  PairingExportFileResult,
  PairingStartResult,
  Permission,
} from './types';
import { applyScenario } from './scenarios';

export type { FakeState } from './fixtures';

export interface FakeIpcOptions {
  now?: () => number;
  delay?: (method: string, params: unknown) => number;
}

type EventHandler = (method: string, params: unknown) => void;
type ConnectionHandler = (connected: boolean) => void;

interface PendingApproval {
  kind: ApprovalKind;
  request: ApprovalRequest;
  timer: ReturnType<typeof setTimeout> | null;
}

const INVALID_PARAMS: RpcErrorShape = { code: -32602, message: 'invalid params' };
const METHOD_NOT_FOUND: RpcErrorShape = { code: -32601, message: 'method not found' };
const PERMISSIONS: readonly Permission[] = ['view_only', 'control', 'unattended'];
const DAY_MS = 24 * 60 * 60 * 1000;

const DEFAULT_SECURITY: NonNullable<HostSettings['security']> = {
  confirm_new_devices: true,
  password_pairing: false,
  password_pairing_permission: 'control',
};

function createDefaultPairFiles(nowMs: number): PairFileInfo[] {
  const states: readonly PairFileState[] = ['active', 'used'];
  return states.map((state, index) => ({
    file_id: index === 0 ? 'AAAAAAAAAAAAAAAAAAAAAA' : 'BBBBBBBBBBBBBBBBBBBBBB',
    label: index === 0 ? 'IT 分发 10 月' : 'iPad 现场支持',
    permission: index === 0 ? 'control' : 'view_only',
    created_at: (nowMs - (index + 1) * DAY_MS) * 1000,
    expires_at: (nowMs + (index === 0 ? DAY_MS : -DAY_MS)) * 1000,
    state,
    used_by_device_id: index === 0 ? null : 'k3jq7x2m9p4tbw8a',
    used_at: index === 0 ? null : (nowMs - DAY_MS) * 1000,
  }));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isEmptyParams(value: unknown): boolean {
  return (
    value === null || value === undefined || (isRecord(value) && Object.keys(value).length === 0)
  );
}

function isPermission(value: unknown): value is Permission {
  return typeof value === 'string' && (PERMISSIONS as readonly string[]).includes(value);
}

function isApprovalKind(value: unknown): value is ApprovalKind {
  return value === 'connection' || value === 'pairing' || value === 'takeover';
}

function notFound(message: string): RpcErrorShape {
  return { code: -32000, message, data: { code: 'not_found' } };
}

function deepMerge(
  target: Record<string, unknown>,
  patch: Record<string, unknown>,
): Record<string, unknown> {
  for (const [key, value] of Object.entries(patch)) {
    if (key === '__proto__' || key === 'constructor' || key === 'prototype') {
      continue;
    }
    const current = target[key];
    if (isRecord(value) && isRecord(current)) {
      target[key] = deepMerge(current, value);
    } else {
      target[key] = structuredClone(value);
    }
  }
  return target;
}

export class FakeIpc implements Transport {
  private readonly now: () => number;
  private readonly delayFor: (method: string, params: unknown) => number;
  private readonly eventHandlers = new Set<EventHandler>();
  private readonly connectionHandlers = new Set<ConnectionHandler>();
  private readonly failures = new Map<MethodName, RpcErrorShape>();
  private readonly pendingApprovals = new Map<string, PendingApproval>();
  private connected = true;
  private pairingCodeIndex = 0;
  private pairFileSeq = 0;
  private readonly pairFiles: PairFileInfo[];

  readonly state: FakeState;
  readonly calls: Array<{ method: string; params: unknown }> = [];
  readonly nowMs: number;

  constructor(init?: Partial<FakeState>, options: FakeIpcOptions = {}) {
    this.now = options.now ?? Date.now;
    this.delayFor = options.delay ?? (() => 0);
    this.nowMs = this.now();
    const defaults = buildFixture(this.nowMs);
    this.state = structuredClone({ ...defaults, ...(init ?? {}) });
    this.state.settings.security = {
      ...DEFAULT_SECURITY,
      ...this.state.settings.security,
    };
    this.pairFiles = createDefaultPairFiles(this.nowMs);
  }

  static fromScenario(name: string | null, options: FakeIpcOptions = {}): FakeIpc {
    const fake = new FakeIpc(undefined, options);
    applyScenario(fake, name);
    return fake;
  }

  call(method: string, params: unknown): Promise<unknown> {
    this.calls.push({ method, params });

    return new Promise<unknown>((resolve, reject) => {
      const execute = (): void => {
        if (!this.connected) {
          reject(structuredClone(AGENT_UNAVAILABLE));
          return;
        }

        const failure = this.failures.get(method as MethodName);
        if (failure) {
          this.failures.delete(method as MethodName);
          reject(structuredClone(failure));
          return;
        }

        try {
          resolve(structuredClone(this.dispatch(method, params)));
        } catch (error) {
          if (this.isRpcError(error)) {
            reject(structuredClone(error));
          } else {
            reject(error);
          }
        }
      };

      const delayMs = this.delayFor(method, params);
      if (delayMs > 0) {
        setTimeout(execute, delayMs);
      } else {
        queueMicrotask(execute);
      }
    });
  }

  failNext(method: MethodName, error: RpcErrorShape): void {
    this.failures.set(method, structuredClone(error));
  }

  emit(method: EventName, params: unknown): void {
    if (method === 'event.approvalRequested') {
      this.trackApprovalRequest(params);
    } else if (method === 'event.approvalClosed') {
      this.releaseApproval(params, false);
    }

    for (const handler of [...this.eventHandlers]) {
      handler(method, params);
    }
  }

  setConnected(connected: boolean): void {
    if (this.connected === connected) {
      return;
    }
    this.connected = connected;
    for (const handler of [...this.connectionHandlers]) {
      handler(connected);
    }
  }

  subscribe(listener: EventHandler): () => void {
    this.eventHandlers.add(listener);
    return () => {
      this.eventHandlers.delete(listener);
    };
  }

  onConnection(listener: ConnectionHandler): () => void {
    this.connectionHandlers.add(listener);
    return () => {
      this.connectionHandlers.delete(listener);
    };
  }

  async isConnected(): Promise<boolean> {
    return this.connected;
  }

  private isRpcError(value: unknown): value is RpcErrorShape {
    return isRecord(value) && typeof value.code === 'number' && typeof value.message === 'string';
  }

  private dispatch(method: string, params: unknown): unknown {
    switch (method) {
      case 'status.get':
        this.requireNoParams(params);
        return { ...this.state.status, sessions: this.state.sessions };
      case 'sessions.list':
        this.requireNoParams(params);
        return { sessions: this.state.sessions };
      case 'sessions.disconnect':
        return this.disconnectSession(params);
      case 'sessions.setViewOnly':
        return this.setSessionViewOnly(params);
      case 'pairing.start':
        this.requireNoParams(params);
        return this.startPairing();
      case 'pairing.cancel':
        if (params !== null && params !== undefined && !isRecord(params)) {
          throw INVALID_PARAMS;
        }
        this.cancelPairing();
        return {};
      case 'pairing.exportFile':
        return this.exportPairFile(params);
      case 'pairing.listFiles':
        this.requireNoParams(params);
        return { files: this.pairFiles };
      case 'pairing.revokeFile':
        return this.revokePairFile(params);
      case 'devices.list':
        this.requireNoParams(params);
        return { devices: this.state.devices };
      case 'devices.update':
        return this.updateDevice(params);
      case 'devices.revoke':
        return this.revokeDevice(params);
      case 'settings.get':
        this.requireNoParams(params);
        return this.state.settings;
      case 'settings.set':
        return this.setSettings(params);
      case 'network.listNics':
        this.requireNoParams(params);
        return { nics: this.state.nics };
      case 'network.firewallStatus':
        this.requireNoParams(params);
        return this.state.firewall;
      case 'network.itNote':
        this.requireNoParams(params);
        return { text: this.state.itNote };
      case 'approval.pending':
        this.requireNoParams(params);
        return this.pendingApprovalRequests();
      case 'approval.respond':
        return this.respondToApproval(params);
      case 'update.status':
        this.requireNoParams(params);
        return this.state.update;
      case 'update.setChannel':
        return this.setUpdateChannel(params);
      case 'update.installNow':
        this.requireNoParams(params);
        this.state.update.state = 'installing';
        this.emit('event.updateState', { state: 'installing' });
        return {};
      case 'logs.export':
        return { path: '/tmp/remote-host-logs.zip' };
      case 'audit.list':
        return this.listAudit(params);
      default:
        throw METHOD_NOT_FOUND;
    }
  }

  private requireNoParams(params: unknown): void {
    if (!isEmptyParams(params)) {
      throw INVALID_PARAMS;
    }
  }

  private sessionId(params: unknown): string {
    if (!isRecord(params) || typeof params.session_id !== 'string') {
      throw INVALID_PARAMS;
    }
    return params.session_id;
  }

  private disconnectSession(params: unknown): Record<string, never> {
    const sessionId = this.sessionId(params);
    const index = this.state.sessions.findIndex((session) => session.session_id === sessionId);
    if (index < 0) {
      throw notFound('session not found');
    }
    this.state.sessions.splice(index, 1);
    this.emit('event.sessionsChanged', { sessions: this.state.sessions });
    return {};
  }

  private setSessionViewOnly(params: unknown): Record<string, never> {
    const sessionId = this.sessionId(params);
    const session = this.state.sessions.find((item) => item.session_id === sessionId);
    if (!session) {
      throw notFound('session not found');
    }
    session.role = 'viewer';
    this.emit('event.sessionsChanged', { sessions: this.state.sessions });
    return {};
  }

  private startPairing(): PairingStartResult {
    const code = PAIRING_CODES[this.pairingCodeIndex % PAIRING_CODES.length] ?? PAIRING_CODES[0];
    this.pairingCodeIndex += 1;
    const result: PairingStartResult = {
      qr_url:
        `rsp://pair?v=1&h=${HOST_ID}&fp=${HOST_FP_B64}&c=${code}` +
        `&n=${HOST_NAME}&a=100.64.0.7:7400`,
      code,
      expires_at: (this.now() + 300_000) * 1000,
    };
    this.state.pairing = result;
    return result;
  }

  private cancelPairing(): void {
    this.state.pairing = null;
    const pairingRequests = [...this.pendingApprovals.entries()]
      .filter(([, approval]) => approval.kind === 'pairing')
      .map(([requestId]) => requestId);
    for (const requestId of pairingRequests) {
      this.closeApproval(requestId, 'cancelled');
    }
  }

  private exportPairFile(params: unknown): PairingExportFileResult {
    if (!isRecord(params)) {
      throw INVALID_PARAMS;
    }
    if (params.permission != null && !isPermission(params.permission)) {
      throw INVALID_PARAMS;
    }
    if (params.label != null && typeof params.label !== 'string') {
      throw INVALID_PARAMS;
    }
    if (params.dir != null && typeof params.dir !== 'string') {
      throw INVALID_PARAMS;
    }

    const permission: Permission = isPermission(params.permission) ? params.permission : 'control';
    this.pairFileSeq += 1;
    const fileId = `EEEEEEEEEEEEEEEEEEEE${String(this.pairFileSeq).padStart(2, '0')}`;
    const fileName = `studio-mac-export-${this.pairFileSeq}.rspair`;
    const now = this.now();
    const result: PairingExportFileResult = {
      file_id: fileId,
      path: `/Users/example/Downloads/${fileName}`,
      file_name: fileName,
      expires_at: (now + DAY_MS) * 1000,
    };
    const file: PairFileInfo = {
      file_id: fileId,
      label: typeof params.label === 'string' ? params.label : null,
      permission,
      created_at: now * 1000,
      expires_at: result.expires_at,
      state: 'active',
      used_by_device_id: null,
      used_at: null,
    };
    this.pairFiles.unshift(file);
    return result;
  }

  private revokePairFile(params: unknown): Record<string, never> {
    if (!isRecord(params) || typeof params.file_id !== 'string') {
      throw INVALID_PARAMS;
    }
    const file = this.pairFiles.find((item) => item.file_id === params.file_id);
    if (!file) {
      throw notFound('pairing file not found');
    }
    file.state = 'revoked';
    return {};
  }

  private updateDevice(params: unknown): Record<string, never> {
    if (!isRecord(params) || !isPermission(params.permission)) {
      throw INVALID_PARAMS;
    }
    if (
      typeof params.device_id !== 'string' ||
      !Array.isArray(params.paths) ||
      !params.paths.every((path): path is string => typeof path === 'string')
    ) {
      throw INVALID_PARAMS;
    }
    const device = this.state.devices.find((item) => item.device_id === params.device_id);
    if (!device) {
      throw notFound('device not found');
    }
    device.permission = params.permission;
    device.paths = [...params.paths];
    return {};
  }

  private revokeDevice(params: unknown): Record<string, never> {
    if (!isRecord(params) || typeof params.device_id !== 'string') {
      throw INVALID_PARAMS;
    }
    const index = this.state.devices.findIndex((item) => item.device_id === params.device_id);
    if (index < 0) {
      throw notFound('device not found');
    }
    this.state.devices.splice(index, 1);
    return {};
  }

  private setSettings(params: unknown): HostSettings {
    if (!isRecord(params)) {
      throw INVALID_PARAMS;
    }
    this.state.settings = deepMerge(
      structuredClone(this.state.settings) as unknown as Record<string, unknown>,
      params,
    ) as unknown as HostSettings;
    const suppliedSecurity = isRecord(params) ? params.security : undefined;
    if (isRecord(suppliedSecurity)) {
      this.state.settings.security = {
        ...DEFAULT_SECURITY,
        ...this.state.settings.security,
        ...suppliedSecurity,
      } as NonNullable<HostSettings['security']>;
    }
    return this.state.settings;
  }

  private respondToApproval(params: unknown): Record<string, never> {
    if (
      !isRecord(params) ||
      typeof params.request_id !== 'string' ||
      typeof params.allow !== 'boolean'
    ) {
      throw INVALID_PARAMS;
    }
    const approval = params as unknown as ApprovalRespondParams;
    this.state.approvals.push(structuredClone(approval));
    this.closeApproval(approval.request_id, approval.allow ? 'allowed' : 'denied');
    return {};
  }

  private pendingApprovalRequests(): ApprovalPendingResult {
    return {
      requests: [...this.pendingApprovals.values()].map((pending) =>
        structuredClone(pending.request),
      ),
    };
  }

  private setUpdateChannel(params: unknown): Record<string, never> {
    if (!isRecord(params) || (params.channel !== 'stable' && params.channel !== 'beta')) {
      throw INVALID_PARAMS;
    }
    this.state.settings.update.channel = params.channel;
    return {};
  }

  private listAudit(params: unknown): { entries: unknown[] } {
    let limit: number | null = null;
    let beforeUs: number | null = null;
    if (params !== null && params !== undefined) {
      if (!isRecord(params)) {
        throw INVALID_PARAMS;
      }
      if (typeof params.limit === 'number') {
        limit = params.limit;
      }
      if (typeof params.before_us === 'number') {
        beforeUs = params.before_us;
      }
    }

    const entries = this.state.audit.filter((entry) => {
      if (beforeUs === null) {
        return true;
      }
      return isRecord(entry) && typeof entry.ts === 'number' && entry.ts < beforeUs;
    });
    const max = limit === null ? 50 : Math.max(0, limit);
    return { entries: entries.slice(0, max) };
  }

  private trackApprovalRequest(params: unknown): void {
    if (
      !isRecord(params) ||
      typeof params.request_id !== 'string' ||
      !isApprovalKind(params.kind)
    ) {
      return;
    }
    this.releaseApproval(params, false);
    const timeoutS = typeof params.timeout_s === 'number' ? params.timeout_s : 30;
    const timer =
      timeoutS > 0
        ? setTimeout(() => {
            this.closeApproval(params.request_id as string, 'timeout');
          }, timeoutS * 1000)
        : null;
    this.pendingApprovals.set(params.request_id, {
      kind: params.kind,
      request: structuredClone(params) as unknown as ApprovalRequest,
      timer,
    });
  }

  private releaseApproval(params: unknown, emitClosed: boolean): void {
    if (!isRecord(params) || typeof params.request_id !== 'string') {
      return;
    }
    const pending = this.pendingApprovals.get(params.request_id);
    if (!pending) {
      return;
    }
    if (emitClosed && isApprovalKind(params.kind) && typeof params.outcome === 'string') {
      this.closeApproval(
        params.request_id,
        params.outcome as IpcEvents['event.approvalClosed']['outcome'],
      );
      return;
    }
    if (pending.timer) {
      clearTimeout(pending.timer);
    }
    this.pendingApprovals.delete(params.request_id);
  }

  private closeApproval(
    requestId: string,
    outcome: IpcEvents['event.approvalClosed']['outcome'],
  ): void {
    const pending = this.pendingApprovals.get(requestId);
    if (!pending) {
      return;
    }
    if (pending.timer) {
      clearTimeout(pending.timer);
    }
    this.pendingApprovals.delete(requestId);
    this.emit('event.approvalClosed', {
      request_id: requestId,
      kind: pending.kind,
      outcome,
    });
  }
}
