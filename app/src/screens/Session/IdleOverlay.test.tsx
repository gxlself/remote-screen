import { StyleSheet } from 'react-native';
import { act, fireEvent } from '@testing-library/react-native';
import { useSessions, type SessionRecord } from '../../features/session/store';
import { liveSession } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { mockWindowDimensions } from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import { IdleOverlay } from './IdleOverlay';
import type { SessionCtx } from './slots';

type Options = NonNullable<Parameters<typeof renderWithProviders>[1]>;

let idleWarning: SessionRecord['idleWarning'] | undefined;

function WarningProbe() {
  idleWarning = useSessions(state => state[liveSession.id]?.idleWarning);
  return null;
}

function record(lockOnEnd: boolean): SessionRecord {
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
    idleWarning: {
      secondsLeft: 60,
      lockOnEnd,
      atUs: 1_792_465_134_000_000,
    },
    controlRequest: null,
    lastError: null,
    clipboard: [],
    phase: 'streaming',
  };
}

function context(
  rec: SessionRecord,
  disconnect: jest.Mock,
  os: 'macos' | 'windows' = 'macos',
): SessionCtx {
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os,
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
    rec,
    view: {
      overlay: 'none',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: true,
      shared: null,
      localUse: false,
      relay: false,
      tcp: false,
      idle: true,
      toolbar: {
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '自动',
        qualityA11y: '画质：自动',
        showCtrlAltDel: os === 'windows',
        showModeSwitch: os === 'macos',
        keyboardEnabled: false,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: 6,
      fps: 60,
      tcp: false,
      alertText: null,
      a11y: '连接质量：直连 6 毫秒，60 帧',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: 0,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect,
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
  };
}

async function mount(
  rec: SessionRecord,
  opts: Options & { disconnect?: jest.Mock } = {},
) {
  const disconnect = opts.disconnect ?? jest.fn();
  const core = opts.core ?? createFakeCore({ sessions: [liveSession] });
  const rendered = await renderWithProviders(
    <>
      <IdleOverlay ctx={context(rec, disconnect)} />
      <WarningProbe />
    </>,
    { ...opts, core },
  );
  return { ...rendered, core, disconnect };
}

async function advance(ms: number) {
  await act(async () => {
    jest.advanceTimersByTime(ms);
  });
}

let renderedIds: Array<{ queryClient: { clear(): void } }> = [];

afterEach(async () => {
  for (const rendered of renderedIds.splice(0)) {
    rendered.queryClient.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

beforeEach(() => {
  jest.useFakeTimers();
});

it('counts down from the warning and explains screen locking', async () => {
  const view = await mount(record(true));
  renderedIds.push(view);

  expect(view.getByRole('header')).toHaveTextContent('长时间没有操作了');
  expect(view.getByText(/并锁定 Mac 屏幕/)).toBeOnTheScreen();
  expect(view.getByLabelText('倒计时 60 秒')).toBeOnTheScreen();
  expect(view.getByTestId('idle-dialog')).toHaveProp(
    'accessibilityViewIsModal',
    true,
  );

  await advance(8000);
  expect(view.getByLabelText('倒计时 52 秒')).toBeOnTheScreen();
});

it('uses the generic lock wording on a Windows host', async () => {
  const rec = record(true);
  const view = await renderWithProviders(
    <IdleOverlay ctx={context(rec, jest.fn(), 'windows')} />,
  );
  renderedIds.push(view);

  expect(view.getByText(/并锁定电脑屏幕/)).toBeOnTheScreen();
  expect(view.queryByText(/Mac/)).toBeNull();
});

it('omits the lock wording when the host will not lock', async () => {
  const view = await mount(record(false));
  renderedIds.push(view);

  expect(view.queryByText(/锁定/)).toBeNull();
  expect(
    view.getByText('要继续保持连接吗？倒计时结束就断开'),
  ).toBeOnTheScreen();
});

it('continues with a Shift tap and clears the warning', async () => {
  const core = createFakeCore({ sessions: [{ ...liveSession }] });
  const view = await mount(record(true), { core });
  renderedIds.push(view);

  await act(() =>
    core.emit({
      type: 'session.idleWarning',
      sessionId: liveSession.id,
      secondsLeft: 60,
      lockOnEnd: true,
    }),
  );
  expect(idleWarning?.secondsLeft).toBe(60);
  await fireEvent.press(view.getByLabelText('继续使用'));

  expect(
    core.calls
      .filter(call => call.method === 'sessions.key')
      .map(call => call.args),
  ).toEqual([
    [liveSession.id, 225, true, 0],
    [liveSession.id, 225, false, 0],
  ]);
  expect(idleWarning).toBeNull();
});

it('disconnects on request', async () => {
  const disconnect = jest.fn();
  const view = await mount(record(true), { disconnect });
  renderedIds.push(view);

  await fireEvent.press(view.getByLabelText('断开'));
  expect(disconnect).toHaveBeenCalledTimes(1);
});

it('does not disconnect by itself at zero remaining', async () => {
  const disconnect = jest.fn();
  const view = await mount(record(true), { disconnect });
  renderedIds.push(view);

  await advance(61_000);
  expect(view.getByLabelText('倒计时 0 秒')).toBeOnTheScreen();
  expect(disconnect).not.toHaveBeenCalled();
  expect(
    view.core.calls.filter(call => call.method === 'sessions.close'),
  ).toEqual([]);
});

it('fits the dialog inside a narrow split-screen window', async () => {
  const restore = mockWindowDimensions({ width: 320, height: 600 });
  try {
    const view = await mount(record(true));
    renderedIds.push(view);
    const dialog = view.getByTestId('idle-dialog');
    expect(StyleSheet.flatten(dialog.props.style).width).toBe(288);
  } finally {
    restore();
  }
});
