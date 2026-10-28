import { HOST_FEATURE_VIRTUAL_DISPLAY } from '../../native/rscore/types';
import type { Participant } from '../../native/rscore/types';
import { sessionHud } from '../../i18n/zh/sessionHud';
import {
  FIXTURE_NOW,
  SELF_DEVICE_ID,
  liveSession,
} from '../../native/rscore/fixtures';
import type { SessionRecord } from './store';
import { deriveSessionView, endRoute } from './view';

const OTHER_DEVICE_ID = 'other-device';

const participant: Participant = {
  deviceId: OTHER_DEVICE_ID,
  deviceName: 'iPad',
  user: 'user@example.com',
  path: 'tailnetDirect',
  since: FIXTURE_NOW,
};

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays: [],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
    ...overrides,
  };
}

const macCtx = {
  os: 'macos' as const,
  quality: 'auto' as const,
  myDeviceId: SELF_DEVICE_ID,
};

describe('endRoute', () => {
  it.each([
    [
      'a live session stays put',
      record({ phase: 'streaming', lastError: { code: 180 } }),
      { kind: 'stay' },
    ],
    [
      'userClosed returns to the device page',
      record({ phase: 'ended', lastError: { reason: 'userClosed' } }),
      { kind: 'goBack' },
    ],
    [
      'background expiry returns with a toast',
      record({ phase: 'ended', lastError: { reason: 'background' } }),
      { kind: 'goBack', toast: sessionHud.end.backgroundExpiredToast },
    ],
    [
      'replaced returns silently',
      record({ phase: 'ended', lastError: { reason: 'replaced' } }),
      { kind: 'goBack' },
    ],
    [
      'hostShutdown returns with a toast',
      record({ phase: 'ended', lastError: { reason: 'hostShutdown' } }),
      { kind: 'goBack', toast: '电脑正在关机或重启' },
    ],
    [
      'hostUpdating stays on the reconnect overlay',
      record({ phase: 'ended', lastError: { reason: 'hostUpdating' } }),
      { kind: 'stay' },
    ],
    [
      'revoked maps to E180',
      record({ phase: 'ended', lastError: { reason: 'revoked' } }),
      { kind: 'error', code: 180 },
    ],
    [
      'permissionLost maps to E101',
      record({ phase: 'ended', lastError: { reason: 'permissionLost' } }),
      { kind: 'error', code: 101 },
    ],
    [
      'protocolError maps to E199',
      record({ phase: 'ended', lastError: { reason: 'protocolError' } }),
      { kind: 'error', code: 199 },
    ],
    [
      'hostClosedByLocalUser uses the general ended page',
      record({
        phase: 'ended',
        lastError: { reason: 'hostClosedByLocalUser' },
      }),
      { kind: 'error', reason: 'hostClosedByLocalUser' },
    ],
    [
      'idleTimeout uses the general ended page',
      record({ phase: 'ended', lastError: { reason: 'idleTimeout' } }),
      { kind: 'error', reason: 'idleTimeout' },
    ],
    [
      'a protocol code keeps its detail',
      record({
        phase: 'ended',
        lastError: { code: 190, detail: { console: 'login_window' } },
      }),
      { kind: 'error', code: 190, detail: { console: 'login_window' } },
    ],
    [
      'a missing reason and code falls back to E199',
      record({ phase: 'ended' }),
      { kind: 'error', code: 199 },
    ],
  ])('%s', (_name, rec, expected) => {
    expect(endRoute(rec)).toEqual(expected);
  });
});

describe('deriveSessionView overlay', () => {
  it.each([
    ['missing record', undefined, 'opening'],
    ['connecting', record({ phase: 'connecting' }), 'connecting'],
    ['authenticating', record({ phase: 'authenticating' }), 'connecting'],
    ['pendingApproval', record({ phase: 'pendingApproval' }), 'approval'],
    ['reconnecting', record({ phase: 'reconnecting' }), 'reconnecting'],
    [
      'hostUpdating after end',
      record({ phase: 'ended', lastError: { reason: 'hostUpdating' } }),
      'reconnecting',
    ],
    [
      'other ended reasons',
      record({ phase: 'ended', lastError: { reason: 'userClosed' } }),
      'none',
    ],
    ['streaming', record({ phase: 'streaming' }), 'none'],
  ] as const)('%s', (_name, rec, expected) => {
    expect(deriveSessionView(rec, macCtx).overlay).toBe(expected);
  });
});

describe('deriveSessionView shared state', () => {
  it('shows the viewer state when another device controls', () => {
    const view = deriveSessionView(
      record({ role: 'viewer', controller: participant }),
      macCtx,
    );
    expect(view.shared).toBe('view');
  });

  it('shows the taken state when a controller has viewers', () => {
    const view = deriveSessionView(
      record({
        role: 'controller',
        controller: { ...participant, deviceId: SELF_DEVICE_ID },
        viewers: [participant],
      }),
      macCtx,
    );
    expect(view.shared).toBe('taken');
  });

  it('shows no shared state for a single participant', () => {
    expect(deriveSessionView(record(), macCtx).shared).toBeNull();
  });
});

describe('deriveSessionView input blocking', () => {
  it.each([
    ['viewer', record({ role: 'viewer' })],
    ['local input pause', record({ inputPausedLocal: true })],
    ['reconnecting', record({ phase: 'reconnecting' })],
  ])('blocks input for %s', (_name, rec) => {
    const view = deriveSessionView(rec, macCtx);
    expect(view.inputBlocked).toBe(true);
    expect(view.toolbar.keyboardEnabled).toBe(false);
  });

  it('leaves input enabled for a regular controller', () => {
    expect(deriveSessionView(record(), macCtx).inputBlocked).toBe(false);
  });
});

describe('deriveSessionView flags', () => {
  it('derives role, local use, relay, TCP and idle indicators', () => {
    const view = deriveSessionView(
      record({
        role: null,
        inputPausedLocal: true,
        path: 'tailnetDerp',
        transport: 'tcp',
        idleWarning: { secondsLeft: 30, lockOnEnd: false, atUs: FIXTURE_NOW },
      }),
      macCtx,
    );
    expect(view).toMatchObject({
      role: 'controller',
      localUse: true,
      relay: true,
      tcp: true,
      idle: true,
    });
  });
});

describe('deriveSessionView toolbar', () => {
  const displays = [
    {
      id: 1,
      name: 'Studio Display',
      widthPx: 2560,
      heightPx: 1600,
      primary: true,
    },
    {
      id: 2,
      name: '竖屏显示器',
      widthPx: 2160,
      heightPx: 3840,
      primary: false,
    },
  ];

  it('shows the second of two displays', () => {
    const view = deriveSessionView(record({ displays, displayId: 2 }), macCtx);
    expect(view.toolbar.displayLabel).toBe('2/2');
    expect(view.toolbar.displayA11y).toBe('切换显示器，当前 2 / 2');
  });

  it('hides the display control for one display', () => {
    const view = deriveSessionView(
      record({ displays: [displays[0]], displayId: 1 }),
      macCtx,
    );
    expect(view.toolbar.displayLabel).toBeNull();
    expect(view.toolbar.displayA11y).toBeNull();
  });

  it('keeps an icon-only display entry for one display when the host can add a virtual display', () => {
    const view = deriveSessionView(
      record({ displays: [displays[0]], displayId: 1, role: 'controller' }),
      { ...macCtx, features: HOST_FEATURE_VIRTUAL_DISPLAY },
    );
    expect(view.toolbar.displayLabel).toBeNull();
    expect(view.toolbar.displayA11y).toBe('显示器和虚拟屏');
  });

  it('hides the single-display entry for a viewer', () => {
    const view = deriveSessionView(
      record({ displays: [displays[0]], displayId: 1, role: 'viewer' }),
      { ...macCtx, features: HOST_FEATURE_VIRTUAL_DISPLAY },
    );
    expect(view.toolbar.displayA11y).toBeNull();
  });

  it('shows the display switch for a Windows host with two displays', () => {
    const view = deriveSessionView(record({ displays, displayId: 1 }), {
      ...macCtx,
      os: 'windows',
    });
    expect(view.toolbar.displayLabel).toBe('1/2');
  });

  it('labels eco quality', () => {
    const view = deriveSessionView(record(), { ...macCtx, quality: 'eco' });
    expect(view.toolbar.qualityLabel).toBe('省流量');
    expect(view.toolbar.qualityA11y).toBe('画质：省流量');
  });

  it('marks a rate-limited 1080p stream', () => {
    const view = deriveSessionView(
      record({
        stats: {
          type: 'session.stats',
          sessionId: liveSession.id,
          rttMs: 48,
          fps: 30,
          bitrateKbps: 1500,
          decodeMs: 8,
          lossPct: 0.5,
          width: 1920,
          height: 1080,
          codec: 'hevc',
          path: 'tailnetDerp',
          transport: 'quic',
          rateReason: 'path:derp',
        },
      }),
      macCtx,
    );
    expect(view.toolbar.qualityLabel).toBe('自动 · 已降低');
    expect(view.toolbar.qualityA11y).toBe('画质：自动，网络不好，已自动降低');
  });

  it('explains the TCP 1080p limit in the accessibility label', () => {
    const view = deriveSessionView(record({ transport: 'tcp' }), macCtx);
    expect(view.toolbar.qualityA11y).toBe(
      '画质：自动，当前网络受限，已自动降低',
    );
  });

  it('shows Windows-only and macOS-only toolbar controls', () => {
    const windows = deriveSessionView(record(), {
      ...macCtx,
      os: 'windows',
    });
    const mac = deriveSessionView(record(), macCtx);
    expect(windows.toolbar.showCtrlAltDel).toBe(true);
    // R17：Windows 主机也有触控板 / 触屏切换。
    expect(windows.toolbar.showModeSwitch).toBe(true);
    expect(mac.toolbar.showCtrlAltDel).toBe(false);
    expect(mac.toolbar.showModeSwitch).toBe(true);
  });
});
