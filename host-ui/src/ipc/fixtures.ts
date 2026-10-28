import type { ApprovalRequest } from './events';
import type {
  ApprovalRespondParams,
  DeviceSummary,
  HostSettings,
  NetworkFirewallStatusResult,
  PairingStartResult,
  SessionSummary,
  StatusGetResult,
  UpdateStatusResult,
} from './types';

export interface FakeState {
  status: Omit<StatusGetResult, 'sessions'>;
  sessions: SessionSummary[];
  devices: DeviceSummary[];
  settings: HostSettings;
  update: UpdateStatusResult;
  nics: unknown[];
  firewall: NetworkFirewallStatusResult;
  itNote: string;
  audit: unknown[];
  pairing: PairingStartResult | null;
  approvals: ApprovalRespondParams[];
}

export const HOST_ID = 'h7v2m9q3k8x4tb1a';
export const HOST_NAME = 'studio-mac';

const IPHONE_ID = 'k3jq7x2m9p4tbw8a';
const IPAD_ID = 'm4ntq8z2c7v5xa1d';
const COLLEAGUE_IPHONE_ID = 'p9wd3k6r2h8sya5e';
const HOUR_MS = 60 * 60 * 1000;

function fingerprint(prefix: readonly number[], suffix: readonly number[]): Uint8Array {
  const bytes = new Uint8Array(32);
  bytes.set(prefix, 0);
  bytes.set(suffix, 28);
  return bytes;
}

const HOST_FP = fingerprint([0x7f, 0x3a, 0x91, 0xc0], [0x5e, 0x2b, 0xc2, 0xd4]);
const DEVICE_FP = fingerprint([0x9c, 0x4e, 0x1b, 0x7a], [0xd0, 0x3f, 0x62, 0xa8]);

export function toB64url(bytes: Uint8Array | readonly number[]): string {
  let binary = '';
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export const HOST_FP_B64 = toB64url(HOST_FP);
export const DEVICE_FP_B64 = toB64url(DEVICE_FP);

export const PAIRING_CODES = ['482913', '173064', '905128'] as const;

export const APPROVAL_PAIRING = {
  request_id: 'r-1',
  kind: 'pairing',
  device_name: '小明的 iPhone',
  device_id: IPHONE_ID,
  platform: 'ios',
  whois_login: 'alice@',
  path: null,
  reasons: [],
  timeout_s: 30,
  permission_choice: true,
  device_fp: DEVICE_FP_B64,
} satisfies ApprovalRequest;

export const APPROVAL_CONNECTION = {
  request_id: 'r-2',
  kind: 'connection',
  device_name: '小明的 iPad',
  device_id: IPAD_ID,
  platform: 'ipados',
  whois_login: 'alice@',
  path: 'tailnet_direct',
  reasons: ['attended'],
  timeout_s: 30,
  permission_choice: false,
  device_fp: DEVICE_FP_B64,
} satisfies ApprovalRequest;

function defaultSettings(): HostSettings {
  return {
    access: {
      allow_remote_control: true,
      allowed_users: ['alice@'],
      attended_mode: false,
      max_viewers: 3,
      idle_timeout_min: 30,
      lock_on_end: false,
      pause_on_local_input: true,
    },
    intranet: {
      enabled: false,
      interfaces: ['en0'],
      allow_sources: ['10.30.0.0/16', '192.168.31.0/24'],
      confirm_each: false,
      allow_pairing: false,
    },
    clipboard: {
      to_phone: true,
      from_phone: true,
    },
    files: {
      enabled: true,
      from_phone: true,
      to_phone: true,
      browse: true,
      dest_dir: '',
      browse_roots: [],
      max_file_mb: 0,
      partial_max_age_days: 7,
    },
    video: {
      codec: 'auto',
      max_fps: 60,
      encoder: 'auto',
    },
    update: {
      channel: 'stable',
      auto_install: true,
    },
    log: {
      level: 'info',
    },
  };
}

export function buildFixture(nowMs: number): FakeState {
  const session: SessionSummary = {
    session_id: 's-1',
    device_id: IPHONE_ID,
    device_name: '小明的 iPhone',
    role: 'controller',
    connected_at_us: (nowMs - 2_538_000) * 1000,
  };
  const devices: DeviceSummary[] = [
    {
      device_id: IPHONE_ID,
      name: '小明的 iPhone',
      permission: 'unattended',
      paths: ['tailnet', 'lan'],
      last_connected_at_us: (nowMs - 2 * HOUR_MS) * 1000,
    },
    {
      device_id: IPAD_ID,
      name: '小明的 iPad',
      permission: 'control',
      paths: ['tailnet', 'lan'],
      last_connected_at_us: (nowMs - 26 * HOUR_MS) * 1000,
    },
    {
      device_id: COLLEAGUE_IPHONE_ID,
      name: '同事的 iPhone',
      permission: 'view_only',
      paths: ['tailnet'],
      last_connected_at_us: null,
    },
  ];

  return {
    status: {
      online: true,
      tailnet_addresses: ['100.64.0.7'],
      version: '0.3.1',
      permissions: {
        screen_recording: true,
        accessibility: true,
      },
      listening: true,
    },
    sessions: [session],
    devices,
    settings: defaultSettings(),
    update: {
      state: 'idle',
      version: null,
      notes: null,
    },
    nics: [
      {
        name: 'en0',
        addresses: ['192.168.31.20'],
        networks: ['192.168.31.0/24'],
        tailnet: false,
      },
      {
        name: 'en1',
        addresses: [],
        networks: [],
        tailnet: false,
      },
      {
        name: 'utun4',
        addresses: ['100.64.0.7'],
        networks: ['100.64.0.0/10'],
        tailnet: true,
      },
    ],
    firewall: {
      enabled: false,
      active: false,
      detail: {},
    },
    itNote:
      'Remote Host 使用 UDP 和 TCP 7400 端口。\n' +
      '请允许来源网段 10.30.0.0/16 与 192.168.31.0/24 访问本机。\n' +
      'tailnet 地址：100.64.0.7；局域网地址：192.168.31.20（en0）。',
    audit: [
      {
        ts: (nowMs - 2 * HOUR_MS) * 1000,
        user: 'alice@',
        device_id: IPHONE_ID,
        event: 'session.end',
        path: 'tailnet_direct',
        duration_s: 2538,
      },
      {
        ts: (nowMs - 2 * HOUR_MS - 2_538_000) * 1000,
        user: 'alice@',
        device_id: IPHONE_ID,
        event: 'session.start',
        path: 'tailnet_direct',
        duration_s: null,
      },
      {
        ts: (nowMs - 26 * HOUR_MS) * 1000,
        user: 'alice@',
        device_id: IPAD_ID,
        event: 'session.start',
        path: 'lan',
        duration_s: null,
      },
    ],
    pairing: null,
    approvals: [],
  };
}
