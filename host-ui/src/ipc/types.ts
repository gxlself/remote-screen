import type { ApprovalRequest } from './events';

export type Permission = 'view_only' | 'control' | 'unattended';
export type SessionRole = 'viewer' | 'controller';
export type UpdateChannel = 'stable' | 'beta';
export type PathName = 'tailnet' | 'intranet' | 'lan';

export interface SessionSummary {
  session_id: string;
  device_id: string;
  device_name: string;
  role: SessionRole;
  connected_at_us: number;
  recording?: boolean;
}

export interface DeviceSummary {
  device_id: string;
  name: string;
  permission: Permission;
  paths: string[];
  last_connected_at_us: number | null;
}

export interface StatusGetResult {
  online: boolean;
  tailnet_addresses: string[];
  version: string;
  permissions: Record<string, unknown>;
  listening: boolean;
  sessions: SessionSummary[];
}

export interface SessionsListResult {
  sessions: SessionSummary[];
}

export interface SessionIdParams {
  session_id: string;
}

export interface PairingStartResult {
  qr_url: string;
  code: string;
  expires_at: number;
}

export interface PairingCancelParams {
  code?: string | null;
}

export interface PairingExportFileParams {
  permission?: Permission | null;
  label?: string | null;
  dir?: string | null;
}

export interface PairingExportFileResult {
  file_id: string;
  path: string;
  file_name: string;
  expires_at: number;
}

export type PairFileState = 'active' | 'used' | 'failed' | 'revoked' | 'expired';

export interface PairFileInfo {
  file_id: string;
  label: string | null;
  permission: Permission;
  created_at: number;
  expires_at: number;
  state: PairFileState;
  used_by_device_id: string | null;
  used_at: number | null;
}

export interface PairingListFilesResult {
  files: PairFileInfo[];
}

export interface PairingRevokeFileParams {
  file_id: string;
}

export interface DevicesListResult {
  devices: DeviceSummary[];
}

export interface DevicesUpdateParams {
  device_id: string;
  permission: Permission;
  paths: string[];
}

export interface DeviceIdParams {
  device_id: string;
}

export interface NetworkListNicsResult {
  nics: unknown[];
}

export interface NetworkFirewallStatusResult {
  enabled: boolean;
  active: boolean;
  detail: unknown;
}

export interface NetworkItNoteResult {
  text: string;
}

export interface ApprovalRespondParams {
  request_id: string;
  allow: boolean;
  permission?: Permission | null;
}

export interface ApprovalPendingResult {
  requests: ApprovalRequest[];
}

export interface UpdateStatusResult {
  state: string;
  version: string | null;
  notes: string | null;
}

export interface UpdateSetChannelParams {
  channel: UpdateChannel;
}

export interface LogsExportParams {
  mask: boolean;
}

export interface LogsExportResult {
  path: string;
}

export interface AuditListParams {
  limit?: number | null;
  before_us?: number | null;
}

export interface AuditListResult {
  entries: unknown[];
}

export type EmptyResult = Record<string, never>;

export interface HostSettings {
  security?: {
    confirm_new_devices: boolean;
    password_pairing: boolean;
    password_pairing_permission: Permission;
  };
  access: {
    allow_remote_control: boolean;
    allowed_users: string[];
    attended_mode: boolean;
    max_viewers: number;
    idle_timeout_min: number;
    lock_on_end: boolean;
    pause_on_local_input: boolean;
  };
  intranet: {
    enabled: boolean;
    interfaces: string[];
    allow_sources: string[];
    confirm_each: boolean;
    allow_pairing: boolean;
  };
  clipboard: {
    to_phone: boolean;
    from_phone: boolean;
  };
  files: {
    enabled: boolean;
    from_phone: boolean;
    to_phone: boolean;
    browse: boolean;
    dest_dir: string;
    browse_roots: string[];
    max_file_mb: number;
    partial_max_age_days: number;
  };
  video: {
    codec: string;
    max_fps: number;
    encoder: string;
  };
  update: {
    channel: UpdateChannel;
    auto_install: boolean;
  };
  log: {
    level: string;
  };
}

export type DeepPartial<T> = T extends readonly unknown[]
  ? T
  : T extends object
    ? { [K in keyof T]?: DeepPartial<T[K]> }
    : T;

export interface IpcMethods {
  'status.get': { params: null; result: StatusGetResult };
  'sessions.list': { params: null; result: SessionsListResult };
  'sessions.disconnect': { params: SessionIdParams; result: EmptyResult };
  'sessions.setViewOnly': { params: SessionIdParams; result: EmptyResult };
  'pairing.start': { params: null; result: PairingStartResult };
  'pairing.cancel': { params: PairingCancelParams; result: EmptyResult };
  'pairing.exportFile': { params: PairingExportFileParams; result: PairingExportFileResult };
  'pairing.listFiles': { params: null; result: PairingListFilesResult };
  'pairing.revokeFile': { params: PairingRevokeFileParams; result: EmptyResult };
  'devices.list': { params: null; result: DevicesListResult };
  'devices.update': { params: DevicesUpdateParams; result: EmptyResult };
  'devices.revoke': { params: DeviceIdParams; result: EmptyResult };
  'settings.get': { params: null; result: HostSettings };
  'settings.set': { params: DeepPartial<HostSettings>; result: HostSettings };
  'network.listNics': { params: null; result: NetworkListNicsResult };
  'network.firewallStatus': { params: null; result: NetworkFirewallStatusResult };
  'network.itNote': { params: null; result: NetworkItNoteResult };
  'approval.pending': { params: null; result: ApprovalPendingResult };
  'approval.respond': { params: ApprovalRespondParams; result: EmptyResult };
  'update.status': { params: null; result: UpdateStatusResult };
  'update.installNow': { params: null; result: EmptyResult };
  'update.setChannel': { params: UpdateSetChannelParams; result: EmptyResult };
  'logs.export': { params: LogsExportParams; result: LogsExportResult };
  'audit.list': { params: AuditListParams; result: AuditListResult };
}

export type MethodName = keyof IpcMethods;
export type Params<M extends MethodName> = IpcMethods[M]['params'];
export type Result<M extends MethodName> = IpcMethods[M]['result'];

export const METHOD_NAMES = [
  'status.get',
  'sessions.list',
  'sessions.disconnect',
  'sessions.setViewOnly',
  'pairing.start',
  'pairing.cancel',
  'pairing.exportFile',
  'pairing.listFiles',
  'pairing.revokeFile',
  'devices.list',
  'devices.update',
  'devices.revoke',
  'settings.get',
  'settings.set',
  'network.listNics',
  'network.firewallStatus',
  'network.itNote',
  'approval.pending',
  'approval.respond',
  'update.status',
  'update.installNow',
  'update.setChannel',
  'logs.export',
  'audit.list',
] as const satisfies readonly MethodName[];

export type ApprovalIntent = 'control' | 'view' | 'files';
