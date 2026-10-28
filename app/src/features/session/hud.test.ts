import { FIXTURE_NOW, liveSession } from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import type { SessionRecord } from './store';
import { hudModel } from './hud';

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

function stats(
  overrides: Partial<CoreEventOf<'session.stats'>> = {},
): CoreEventOf<'session.stats'> {
  return {
    type: 'session.stats',
    sessionId: liveSession.id,
    rttMs: 6,
    fps: 60,
    bitrateKbps: 8000,
    decodeMs: 3,
    lossPct: 0,
    width: 2560,
    height: 1440,
    codec: 'hevc',
    path: 'tailnetDirect',
    transport: 'quic',
    ...overrides,
  };
}

it('describes a direct connection', () => {
  expect(hudModel(record({ stats: stats() }))).toMatchObject({
    path: 'tailnetDirect',
    pathText: '直连',
    latencyMs: 6,
    fps: 60,
    tcp: false,
    alertText: null,
    a11y: '连接质量：直连 6 毫秒，60 帧，查看详情',
  });
});

it('describes the Shanghai relay', () => {
  expect(
    hudModel(
      record({
        stats: stats({
          rttMs: 48,
          fps: 30,
          path: 'tailnetDerp',
          derpRegion: '上海',
        }),
      }),
    ),
  ).toMatchObject({
    path: 'tailnetDerp',
    pathText: '中继 · 上海',
    latencyMs: 48,
    fps: 30,
    a11y: '连接质量：经上海中继，48 毫秒，30 帧，查看详情',
  });
});

it('describes TCP over the company intranet', () => {
  expect(
    hudModel(
      record({
        stats: stats({
          rttMs: 23,
          fps: 30,
          path: 'intranet',
          transport: 'tcp',
        }),
      }),
    ),
  ).toMatchObject({
    path: 'intranet',
    pathText: '内网 · TCP',
    latencyMs: 23,
    fps: 30,
    tcp: true,
    a11y: '连接质量：经公司内网，TCP 模式，23 毫秒，30 帧，查看详情',
  });
});

it('uses the recorded path while statistics are unavailable', () => {
  expect(hudModel(record({ path: 'tailnetDirect' }))).toMatchObject({
    path: 'tailnetDirect',
    pathText: '直连',
    latencyMs: null,
    fps: null,
    tcp: false,
    alertText: null,
    a11y: '连接质量：正在测量，查看详情',
  });
});

it('turns a rate-limit reason into the canvas downgrade notice', () => {
  expect(
    hudModel(
      record({
        stats: stats({
          height: 1080,
          fps: 30,
          path: 'tailnetDerp',
          derpRegion: '上海',
          rateReason: 'path:derp',
        }),
      }),
    ).alertText,
  ).toBe('画质已自动降低');
});

it('omits the relay region when Host did not report one', () => {
  expect(
    hudModel(
      record({ stats: stats({ path: 'tailnetDerp', derpRegion: undefined }) }),
    ).pathText,
  ).toBe('中继');
});

it('uses the latest statistics path and transport', () => {
  expect(
    hudModel(
      record({
        path: 'tailnetDirect',
        transport: 'quic',
        stats: stats({ path: 'intranet', transport: 'tcp' }),
      }),
    ),
  ).toMatchObject({ path: 'intranet', pathText: '内网 · TCP', tcp: true });
});

it('keeps the idle receipt timestamp out of the HUD model', () => {
  const model = hudModel(
    record({
      idleWarning: { secondsLeft: 30, lockOnEnd: false, atUs: FIXTURE_NOW },
      stats: stats(),
    }),
  );
  expect(model).not.toHaveProperty('idle');
});
