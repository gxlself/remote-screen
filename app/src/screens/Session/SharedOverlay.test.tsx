import { act, cleanup, fireEvent } from '@testing-library/react-native';
import { Text } from 'react-native';
import type { SessionCtx } from './slots';
import { REQUEST_WAIT_MS, SharedOverlay } from './SharedOverlay';
import { renderWithProviders } from '../../test/render';
import { mockWindowDimensions } from '../../test/dimensions';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  SELF_DEVICE_ID,
  liveSession,
} from '../../native/rscore/fixtures';
import type { Participant } from '../../native/rscore/types';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { useSessions } from '../../features/session/store';
import type { SessionRecord } from '../../features/session/store';

const SID = 's1';
const IPHONE_ID = 'k3jq7x2m9p4tbw8a';
const MY_USER = 'alice@';
const IPAD: Participant = {
  deviceId: SELF_DEVICE_ID,
  deviceName: '小明的 iPad',
  user: MY_USER,
  path: 'tailnetDirect',
  since: FIXTURE_NOW,
};
const ALICE_IPAD: Participant = {
  ...IPAD,
  deviceId: 'other-device',
  user: 'friend@example.com',
};
// The Host lists this phone among the viewers with the login it saw for it.
const MY_IPHONE: Participant = {
  ...IPAD,
  deviceId: IPHONE_ID,
  deviceName: '小明的 iPhone',
};

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    id: SID,
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

type TestOverrides = Omit<Partial<SessionCtx>, 'showToast'> & {
  showToast?: jest.Mock<
    ReturnType<SessionCtx['showToast']>,
    Parameters<SessionCtx['showToast']>
  >;
};

type TestCtx = SessionCtx & {
  showToast: jest.Mock<
    ReturnType<SessionCtx['showToast']>,
    Parameters<SessionCtx['showToast']>
  >;
};

function ctx(overrides: TestOverrides = {}): TestCtx {
  const showToast = jest.fn<
    ReturnType<SessionCtx['showToast']>,
    Parameters<SessionCtx['showToast']>
  >();
  return {
    hostId: liveSession.hostId,
    hostName: liveSession.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: IPHONE_ID,
    videoRef: { current: null },
    sessionId: SID,
    rec: record({ role: 'controller' }),
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
    showToast,
    ...overrides,
  };
}

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

async function mount(value: SessionCtx, core: FakeCore) {
  const view = await renderWithProviders(<SharedOverlay ctx={value} />, {
    core,
  });
  await flush();
  return view;
}

async function emitControlRequest(core: FakeCore, timeoutS = 30) {
  await act(() =>
    core.emit({
      type: 'session.controlRequest',
      sessionId: SID,
      requester: IPAD,
      timeoutS,
    }),
  );
  await flush();
}

function ControlRequestProbe() {
  const request = useSessions(state => state[SID]?.controlRequest ?? null);
  return <Text>{request === null ? 'store-null' : 'store-present'}</Text>;
}

function requestRecord() {
  return record({
    role: 'controller',
    controlRequest: {
      requester: IPAD,
      timeoutS: 30,
      atUs: FIXTURE_NOW,
    },
  });
}

it('shows the takeover bar for a viewer and requests control', async () => {
  const core = createFakeCore({ sessions: [] });
  const view = await mount(
    ctx({
      rec: record({ role: 'viewer', controller: IPAD, viewers: [MY_IPHONE] }),
    }),
    core,
  );
  expect(view.getByText('小明的 iPad 正在控制')).toBeOnTheScreen();
  expect(
    view.getByText('你在观看 · 都是你的设备，接管不用确认'),
  ).toBeOnTheScreen();

  await fireEvent.press(view.getByText('接管控制'));
  expect(core.calls).toContainEqual({
    method: 'sessions.requestControl',
    args: [SID],
  });
  view.queryClient.clear();
});

it('asks for consent when the controller belongs to another user', async () => {
  const core = createFakeCore({ sessions: [] });
  const view = await mount(
    ctx({
      rec: record({
        role: 'viewer',
        controller: ALICE_IPAD,
        viewers: [MY_IPHONE],
      }),
    }),
    core,
  );
  expect(view.getByText('你在观看 · 接管需要对方同意')).toBeOnTheScreen();
  expect(view.getByText('请求控制')).toBeOnTheScreen();
  view.queryClient.clear();
});

it('asks for consent when the Host gives no login for this phone', async () => {
  const core = createFakeCore({ sessions: [] });
  const unknown = await mount(
    ctx({ rec: record({ role: 'viewer', controller: IPAD, viewers: [] }) }),
    core,
  );
  expect(unknown.getByText('你在观看 · 接管需要对方同意')).toBeOnTheScreen();
  expect(unknown.getByText('请求控制')).toBeOnTheScreen();
  unknown.queryClient.clear();
  await unknown.unmount();

  const blank = await mount(
    ctx({
      rec: record({
        role: 'viewer',
        controller: { ...IPAD, user: '' },
        viewers: [{ ...MY_IPHONE, user: '' }],
      }),
    }),
    core,
  );
  expect(blank.getByText('请求控制')).toBeOnTheScreen();
  expect(blank.queryByText('接管控制')).toBeNull();
  blank.queryClient.clear();
});

it('shows the handoff card after becoming controller, then auto-dismisses it', async () => {
  const core = createFakeCore({ sessions: [] });
  const view = await mount(
    ctx({ rec: record({ role: 'viewer', controller: IPAD }) }),
    core,
  );

  await view.rerender(
    <SharedOverlay
      ctx={ctx({ rec: record({ role: 'controller', viewers: [IPAD] }) })}
    />,
  );
  await flush();
  expect(view.getByText('已从 iPad 接管，现在由你控制')).toBeOnTheScreen();
  expect(
    view.getByText('iPad 现在只能观看，它那边也收到了提示'),
  ).toBeOnTheScreen();
  expect(view.getByText('小明的 iPad 在观看')).toBeOnTheScreen();

  await act(async () => {
    await jest.advanceTimersByTimeAsync(8000);
  });
  expect(view.queryByText('已从 iPad 接管，现在由你控制')).toBeNull();
  expect(view.getByText('小明的 iPad 在观看')).toBeOnTheScreen();
  view.queryClient.clear();
});

it('clears a control request after agreeing', async () => {
  const core = createFakeCore({
    sessions: [{ ...liveSession, id: SID, role: 'controller' }],
  });
  const value = ctx({ rec: requestRecord() });
  const view = await renderWithProviders(
    <>
      <SharedOverlay ctx={value} />
      <ControlRequestProbe />
    </>,
    { core },
  );
  await flush();
  await emitControlRequest(core);
  expect(view.getByText('小明的 iPad 想接管控制')).toBeOnTheScreen();
  expect(
    view.getByText('同意后你改为观看。30 秒内不答复就算拒绝。'),
  ).toBeOnTheScreen();
  expect(view.getByText('store-present')).toBeOnTheScreen();

  await fireEvent.press(view.getByText('同意'));
  expect(core.calls).toContainEqual({
    method: 'sessions.respondControl',
    args: [SID, SELF_DEVICE_ID, true],
  });
  view.queryClient.clear();
});

it('toasts when respondControl is unsupported', async () => {
  const core = createFakeCore({
    sessions: [{ ...liveSession, id: SID, role: 'controller' }],
  });
  core.failNext(
    'sessions.respondControl',
    new RsCoreError('unsupported', 'not implemented'),
  );
  const value = ctx({ rec: requestRecord() });
  const view = await mount(value, core);
  await fireEvent.press(view.getByText('拒绝'));
  expect(value.showToast).toHaveBeenCalledWith(
    '这个版本还不能答复，对方会在超时后被拒绝',
  );
  view.queryClient.clear();
});

it('answers a request only once even when tapped twice', async () => {
  const core = createFakeCore({
    sessions: [{ ...liveSession, id: SID, role: 'controller' }],
  });
  const value = ctx({ rec: requestRecord() });
  const view = await renderWithProviders(<SharedOverlay ctx={value} />, {
    core,
  });
  await flush();
  await emitControlRequest(core);

  const agree = view.getByText('同意');
  await fireEvent.press(agree);
  await fireEvent.press(agree);
  expect(
    core.calls.filter(call => call.method === 'sessions.respondControl'),
  ).toHaveLength(1);
  view.queryClient.clear();
});

it('does not respond when the request countdown reaches zero', async () => {
  const core = createFakeCore({
    sessions: [{ ...liveSession, id: SID, role: 'controller' }],
  });
  const value = ctx({ rec: requestRecord() });
  const view = await renderWithProviders(
    <>
      <SharedOverlay ctx={value} />
      <ControlRequestProbe />
    </>,
    { core },
  );
  await flush();
  await emitControlRequest(core);
  expect(view.getByText('store-present')).toBeOnTheScreen();
  await act(async () => {
    await jest.advanceTimersByTimeAsync(30000);
  });
  expect(view.getByText('store-null')).toBeOnTheScreen();
  expect(
    core.calls.some(call => call.method === 'sessions.respondControl'),
  ).toBe(false);
  view.queryClient.clear();
});

it('shows a rejection toast once for the same error object', async () => {
  const core = createFakeCore({ sessions: [] });
  const error = { code: 141, message: 'denied' };
  const value = ctx({ rec: record({ role: 'viewer', lastError: error }) });
  const view = await mount(value, core);
  expect(value.showToast).toHaveBeenCalledTimes(1);
  expect(value.showToast).toHaveBeenCalledWith('接管请求被拒绝', {
    tone: 'bad',
  });
  await view.rerender(<SharedOverlay ctx={value} />);
  await flush();
  expect(value.showToast).toHaveBeenCalledTimes(1);
  view.queryClient.clear();
});

it('shows the view-only bar without a controller', async () => {
  const core = createFakeCore({ sessions: [] });
  const view = await mount(
    ctx({ rec: record({ role: 'viewer', controller: null }) }),
    core,
  );
  expect(view.getByText('仅观看：触摸和键盘不会发到电脑')).toBeOnTheScreen();
  expect(view.queryByText('接管控制')).toBeNull();
  view.queryClient.clear();
});

describe('R18 waiting for a control request', () => {
  function viewerCtx(overrides: Partial<SessionRecord> = {}) {
    return ctx({
      rec: record({
        role: 'viewer',
        controller: ALICE_IPAD,
        viewers: [MY_IPHONE],
        ...overrides,
      }),
    });
  }

  function requests(core: FakeCore) {
    return core.calls.filter(call => call.method === 'sessions.requestControl');
  }

  it('waits after asking and ignores repeated taps', async () => {
    const core = createFakeCore({ sessions: [] });
    const view = await mount(viewerCtx(), core);
    await fireEvent.press(view.getByTestId('shared-take-over'));
    expect(requests(core)).toHaveLength(1);
    expect(view.getByTestId('shared-bar-status')).toHaveTextContent(
      '已发出请求，等 小明的 iPad 同意',
    );
    expect(view.getByTestId('shared-take-over-spinner')).toBeOnTheScreen();
    expect(view.getByTestId('shared-take-over')).toBeDisabled();

    await fireEvent.press(view.getByTestId('shared-take-over'));
    expect(requests(core)).toHaveLength(1);
    view.queryClient.clear();
  });

  it('says so when no answer arrives and lets the user ask again', async () => {
    const core = createFakeCore({ sessions: [] });
    const value = viewerCtx();
    const view = await mount(value, core);
    await fireEvent.press(view.getByTestId('shared-take-over'));
    await act(async () => {
      await jest.advanceTimersByTimeAsync(REQUEST_WAIT_MS);
    });
    expect(value.showToast).toHaveBeenCalledWith(
      '一直没收到答复，可以再请求一次',
      { tone: 'bad' },
    );
    expect(view.getByTestId('shared-take-over')).not.toBeDisabled();
    await fireEvent.press(view.getByTestId('shared-take-over'));
    expect(requests(core)).toHaveLength(2);
    view.queryClient.clear();
  });

  it('explains why the Host turned the request down', async () => {
    const core = createFakeCore({ sessions: [] });
    const value = viewerCtx();
    const view = await mount(value, core);
    await fireEvent.press(view.getByTestId('shared-take-over'));

    const denied = {
      ...value,
      rec: record({
        role: 'viewer',
        controller: ALICE_IPAD,
        viewers: [MY_IPHONE],
        lastError: { code: 141, detail: { reason: 'timeout' } },
      }),
    };
    await view.rerender(<SharedOverlay ctx={denied} />);
    await flush();
    expect(value.showToast).toHaveBeenCalledWith(
      '对方没有答复，接管请求已取消',
      { tone: 'bad' },
    );
    expect(view.getByTestId('shared-take-over')).not.toBeDisabled();
    expect(view.getByText('你在观看 · 接管需要对方同意')).toBeOnTheScreen();

    // 收到结果后不会再报「没收到答复」。
    await act(async () => {
      await jest.advanceTimersByTimeAsync(REQUEST_WAIT_MS);
    });
    expect(value.showToast).toHaveBeenCalledTimes(1);
    view.queryClient.clear();
  });

  it('stops waiting once control is granted', async () => {
    const core = createFakeCore({ sessions: [] });
    const value = viewerCtx();
    const view = await mount(value, core);
    await fireEvent.press(view.getByTestId('shared-take-over'));
    await view.rerender(
      <SharedOverlay
        ctx={{
          ...value,
          rec: record({ role: 'controller', controller: null }),
        }}
      />,
    );
    await act(async () => {
      await jest.advanceTimersByTimeAsync(REQUEST_WAIT_MS);
    });
    expect(value.showToast).not.toHaveBeenCalled();
    view.queryClient.clear();
  });

  it('reports a request that could not be sent', async () => {
    const core = createFakeCore({ sessions: [] });
    core.failNext('sessions.requestControl', new Error('closed'));
    const value = viewerCtx();
    const view = await mount(value, core);
    await fireEvent.press(view.getByTestId('shared-take-over'));
    expect(value.showToast).toHaveBeenCalledWith('请求没发出去，请重试', {
      tone: 'bad',
    });
    expect(view.getByTestId('shared-take-over')).not.toBeDisabled();
    view.queryClient.clear();
  });
});
