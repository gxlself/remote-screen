import { GAP_METHODS, RSP_DEFAULT_PORT, RsCoreError } from './RsCoreApi';
import type {
  Computer,
  CoreEvent,
  CoreEventOf,
  HostUpdateState,
} from './types';

// 只由 tsc 验证，不执行这些赋值；@ts-expect-error 也必须由 typecheck 核对。
export function checkTypeAssignments(stats: CoreEventOf<'session.stats'>) {
  const computer: Computer = {
    hostId: 'host-1',
    name: 'Example Mac',
    node: 'example-mac',
    os: 'macos',
    osVersion: '15.0',
    online: true,
    hostRunning: true,
    lastSeenAt: 1_792_465_134_000_000,
    lastHeartbeatAt: null,
    hostVersion: '0.1.0',
    features: 0,
    fp: 'example-fingerprint',
    addresses: [
      { kind: 'tailnet', addr: '100.64.0.7', port: 7400 },
      {
        kind: 'lan',
        addr: '192.168.31.7',
        port: 7400,
        cidr: '192.168.31.0/24',
      },
    ],
    displays: [
      { id: 1, name: 'Built-in', widthPx: 1920, heightPx: 1080, primary: true },
    ],
    permsOk: true,
    sessions: [
      {
        deviceName: 'Example iPhone',
        role: 'controller',
        since: 1_792_465_134_000_000,
      },
    ],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: 'https://hs.example.com/v1/hosts/host-1/thumbnail',
    filevault: false,
    console: { loggedIn: true, locked: false, loginWindow: false },
  };
  const { hostId, ...withoutHostId } = computer;
  // @ts-expect-error Computer 必须包含 hostId。
  const missingHostId: Computer = withoutHostId;
  const rttMs: number = stats.rttMs;
  // @ts-expect-error session.stats 必须包含统计字段，且没有 foo。
  const invalidEvent: CoreEvent = { type: 'session.stats', foo: 1 };
  return { hostId, missingHostId, rttMs, invalidEvent };
}

export function checkExhaustiveEvent(event: CoreEvent): void {
  switch (event.type) {
    case 'file.transfer':
    case 'file.transferRemoved':
    case 'session.state':
      return;
    case 'session.role':
      return;
    case 'session.stats':
      return;
    case 'session.error':
      return;
    case 'session.displays':
    case 'session.vdisplay':
      return;
    case 'session.clipboard':
      return;
    case 'session.idleWarning':
      return;
    case 'session.controlRequest':
      return;
    case 'pairing.progress':
      return;
    case 'pairing.done':
      return;
    case 'probe.result':
      return;
    case 'discover.found':
    case 'discover.done':
      return;
    case 'ssh.state':
      return;
    case 'ssh.hostKeyPrompt':
      return;
    case 'ssh.authUrl':
      return;
    case 'tailnet.state':
      return;
    case 'adapter.state':
    case 'adapter.certPrompt':
    case 'adapter.clipboard':
    case 'adapter.clipboardOther':
    case 'adapter.screens':
      return;
    case 'network.changed':
    case 'recording.state':
    case 'playback.state':
      return;
    default: {
      const _x: never = event;
      return _x;
    }
  }
}

it('extracts tailnet.state from CoreEvent', () => {
  const event: CoreEventOf<'tailnet.state'> = {
    type: 'tailnet.state',
    state: 'needsLogin',
    authUrl: 'https://hs.example.com/register/nodekey-test',
    ips: [],
    derpRegion: '',
    message: null,
  };
  expect(event.type).toBe('tailnet.state');
});

it('lists the contract gap methods as qualified names', () => {
  expect(GAP_METHODS).toHaveLength(17);
  expect(GAP_METHODS).not.toContain('hub.me');
  for (const method of GAP_METHODS) {
    expect(method).toMatch(/^[a-z]+\.[a-zA-Z]+$/);
  }
});

it('preserves conflict details on an Error instance', () => {
  const error = new RsCoreError('conflict', 'x', 409, 'conflict', {
    version: 3,
    value: [],
  });
  expect(error.name).toBe('RsCoreError');
  expect(error).toBeInstanceOf(Error);
  expect(error).toBeInstanceOf(RsCoreError);
  expect(error.kind).toBe('conflict');
  expect(error.message).toBe('x');
  expect(error.httpStatus).toBe(409);
  expect(error.hubCode).toBe('conflict');
  expect(error.current?.version).toBe(3);
});

it('lists every host update state once in contract order', () => {
  const states: HostUpdateState[] = [
    'idle',
    'waitingSessionEnd',
    'downloading',
    'verifying',
    'installing',
    'restarting',
    'ready',
    'rolledBack',
    'failed',
  ];
  expect(states).toHaveLength(9);
  expect(new Set(states).size).toBe(9);
});

it('uses the RSP default port', () => {
  expect(RSP_DEFAULT_PORT).toBe(7400);
});
