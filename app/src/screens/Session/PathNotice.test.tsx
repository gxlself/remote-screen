import { act, cleanup, fireEvent } from '@testing-library/react-native';
import type { SessionCtx } from './slots';
import { PathCaption, PathNotice } from './PathNotice';
import { tokens } from '../../theme/tokens';

/** 会话页里两者一起出现：说明在顶部提示栏，卡片在底部。 */
function PathSlots({ ctx: value }: { ctx: SessionCtx }) {
  return (
    <>
      <PathCaption ctx={value} />
      <PathNotice ctx={value} />
    </>
  );
}
import { renderWithProviders } from '../../test/render';
import { mockWindowDimensions } from '../../test/dimensions';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import type { SessionRecord } from '../../features/session/store';

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
    rttMs: 48,
    fps: 30,
    bitrateKbps: 3000,
    decodeMs: 4,
    lossPct: 0,
    width: 1920,
    height: 1080,
    codec: 'hevc',
    path: 'tailnetDerp',
    transport: 'quic',
    derpRegion: '上海',
    ...overrides,
  };
}

function ctx(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: liveSession.hostId,
    hostName: liveSession.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: 'k3jq7x2m9p4tbw8a',
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: record(),
    view: {
      overlay: 'none',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: false,
      shared: null,
      localUse: false,
      relay: false,
      tcp: false,
      idle: false,
      toolbar: {
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '',
        qualityA11y: '',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: null,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect: jest.fn(),
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
    ...overrides,
  };
}

const networks = [
  {
    id: 'company',
    name: '公司 VPN',
    cidrs: ['10.30.0.0/16'],
    vpnPools: ['10.99.0.0/16'],
  },
];

let restoreDimensions: (() => void) | null = null;

beforeEach(() => {
  jest.useFakeTimers();
  restoreDimensions = mockWindowDimensions({ width: 1024, height: 768 });
});

afterEach(async () => {
  await cleanup();
  restoreDimensions?.();
  restoreDimensions = null;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await jest.advanceTimersByTimeAsync(1);
  });
}

it('shows the relay caption and relay notice with current stats', async () => {
  const core = createFakeCore({ sessions: [] });
  const relayCtx = ctx({
    rec: record({
      path: 'tailnetDerp',
      derpRegion: '上海',
      stats: stats(),
    }),
    view: {
      ...ctx().view,
      relay: true,
    },
  });
  const view = await renderWithProviders(<PathSlots ctx={relayCtx} />, {
    core,
    settings: { networks },
  });
  await flush();
  expect(view.getByText('后台继续尝试直连')).toBeOnTheScreen();
  // R38：说明带底色，不直接压在画面上。
  expect(view.getByTestId('path-caption')).toHaveStyle({
    backgroundColor: tokens.hud,
  });
  expect(view.getByText('经上海中继转发，画质已自动降低')).toBeOnTheScreen();
  expect(
    view.getByText('能直接连上电脑时会自动换回来，不用手动切'),
  ).toBeOnTheScreen();
  view.queryClient.clear();
});

it('dismisses the relay notice and does not show it again', async () => {
  const core = createFakeCore({ sessions: [] });
  const value = ctx({
    rec: record({ path: 'tailnetDerp', derpRegion: '上海', stats: stats() }),
    view: { ...ctx().view, relay: true },
  });
  const view = await renderWithProviders(<PathSlots ctx={value} />, { core });
  await flush();
  await fireEvent.press(view.getByText('知道了'));
  expect(view.queryByText('经上海中继转发，画质已自动降低')).toBeNull();

  await view.rerender(<PathSlots ctx={value} />);
  await flush();
  expect(view.queryByText('经上海中继转发，画质已自动降低')).toBeNull();
  view.queryClient.clear();
});

it('shows a TCP notice for the company network and auto-dismisses it', async () => {
  const core = createFakeCore({ sessions: [] });
  const value = ctx({
    rec: record({
      path: 'intranet',
      transport: 'tcp',
      dial: [{ path: 'intranet', addr: '10.30.12.48', state: 'ok' }],
    }),
    view: { ...ctx().view, tcp: true },
  });
  const view = await renderWithProviders(<PathSlots ctx={value} />, {
    core,
    settings: { networks },
  });
  await flush();
  expect(view.getByText('经公司 VPN · 10.30.12.48')).toBeOnTheScreen();
  expect(
    view.getByText('公司网络限制了快速通道，已换成备用连接'),
  ).toBeOnTheScreen();
  expect(
    view.getByText('画面会比平时糊一点，换个网络后会自动换回来'),
  ).toBeOnTheScreen();

  await act(async () => {
    await jest.advanceTimersByTimeAsync(8000);
  });
  expect(view.queryByText('公司网络限制了快速通道，已换成备用连接')).toBeNull();
  view.queryClient.clear();
});

it('uses the generic TCP copy outside the company network', async () => {
  const core = createFakeCore({ sessions: [] });
  const value = ctx({
    rec: record({
      path: 'lan',
      transport: 'tcp',
      dial: [{ path: 'lan', addr: '192.168.31.20', state: 'ok' }],
    }),
    view: { ...ctx().view, tcp: true },
  });
  const view = await renderWithProviders(<PathSlots ctx={value} />, {
    core,
    settings: { networks },
  });
  await flush();
  expect(
    view.getByText('当前网络限制了快速通道，已换成备用连接'),
  ).toBeOnTheScreen();
  view.queryClient.clear();
});

it('uses the ok dial attempt rather than a failed one', async () => {
  const core = createFakeCore({ sessions: [] });
  const value = ctx({
    rec: record({
      path: 'intranet',
      transport: 'tcp',
      dial: [
        { path: 'intranet', addr: '10.30.99.1', state: 'failed' },
        { path: 'intranet', addr: '10.30.12.48', state: 'ok' },
      ],
    }),
    view: { ...ctx().view, tcp: true },
  });
  const view = await renderWithProviders(<PathSlots ctx={value} />, {
    core,
    settings: { networks },
  });
  await flush();
  expect(view.getByText('经公司 VPN · 10.30.12.48')).toBeOnTheScreen();
  view.queryClient.clear();
});

it('only shows the TCP caption when the transport is TCP', async () => {
  const core = createFakeCore({ sessions: [] });
  const view = await renderWithProviders(
    <PathCaption
      ctx={ctx({
        rec: record({ path: 'intranet', transport: 'quic' }),
        view: { ...ctx().view, relay: true },
      })}
    />,
    { core, settings: { networks } },
  );
  await flush();
  expect(view.queryByText('经公司 VPN · 10.30.12.48')).toBeNull();
  view.queryClient.clear();
});

it('shows relay and TCP notices one after another', async () => {
  const core = createFakeCore({ sessions: [] });
  const relayCtx = ctx({
    rec: record({ path: 'tailnetDerp', derpRegion: '上海', stats: stats() }),
    view: { ...ctx().view, relay: true },
  });
  const view = await renderWithProviders(<PathSlots ctx={relayCtx} />, {
    core,
    settings: { networks },
  });
  await flush();
  await view.rerender(
    <PathSlots
      ctx={ctx({
        rec: record({
          path: 'intranet',
          transport: 'tcp',
          dial: [{ path: 'intranet', addr: '10.30.12.48', state: 'ok' }],
        }),
        view: { ...ctx().view, tcp: true },
      })}
    />,
  );
  await flush();
  expect(view.getByText('经公司 VPN · 10.30.12.48')).toBeOnTheScreen();
  expect(view.getByText('经中继转发，画质会自动降低')).toBeOnTheScreen();

  await fireEvent.press(view.getByText('知道了'));
  await flush();
  expect(view.queryByText('经中继转发，画质会自动降低')).toBeNull();
  expect(
    view.getByText('公司网络限制了快速通道，已换成备用连接'),
  ).toBeOnTheScreen();
  view.queryClient.clear();
});
