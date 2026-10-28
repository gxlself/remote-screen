import { act, fireEvent, screen } from '@testing-library/react-native';
import { computers, liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { connectPath, ConnectingOverlay } from './ConnectingOverlay';
import type { SessionCtx } from './slots';

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: computers[0].hostId,
    hostName: computers[0].name,
    os: 'macos',
    host: computers[0],
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: {
      ...liveSession,
      phase: 'connecting',
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
    },
    view: {
      overlay: 'connecting',
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
        qualityLabel: '自动',
        qualityA11y: '画质自动',
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

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(0);
});

afterEach(() => {
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('ConnectingOverlay', () => {
  it('shows the title and the last active dial path', async () => {
    const rec = context().rec;
    expect(rec).toBeDefined();
    await renderWithProviders(
      <ConnectingOverlay
        ctx={context({
          rec: {
            ...rec!,
            dial: [
              { path: 'lan', addr: '192.168.31.20', state: 'failed' },
              {
                path: 'tailnetDirect',
                addr: '100.64.0.7',
                state: 'trying',
              },
            ],
          },
        })}
      />,
    );

    expect(screen.getByText('正在连接 Studio Mac mini')).toBeOnTheScreen();
    expect(screen.getByText('直连')).toBeOnTheScreen();
    expect(screen.queryByText(/100\.64\.0\.7|studio-mac/)).toBeNull();
    expect(screen.getByLabelText('连接步骤')).toBeOnTheScreen();
  });

  it('switches from the auth step to the first-frame stuck notice', async () => {
    const base = context();
    await renderWithProviders(
      <ConnectingOverlay
        ctx={context({
          rec: { ...base.rec!, phase: 'streaming' },
          phaseAtMs: { authenticating: 400, streaming: 700 },
        })}
      />,
    );

    await advance(3400);
    expect(screen.queryByTestId('connect-stuck')).toBeNull();
    await advance(5000);
    expect(
      screen.getByText('已等 8 秒，还在等画面：Mac 可能刚从睡眠里醒来'),
    ).toBeOnTheScreen();
    expect(screen.getByText('不用操作，画面一到就自动进入')).toBeOnTheScreen();
  });

  it('uses 电脑 instead of Mac for a Windows host stuck on the tunnel', async () => {
    const base = context();
    await renderWithProviders(
      <ConnectingOverlay
        ctx={context({
          os: 'windows',
          hostName: '工作室 Windows',
          rec: { ...base.rec!, phase: 'connecting' },
          startedAtMs: 0,
        })}
      />,
    );

    await advance(8000);
    expect(
      screen.getByText('已等 8 秒，还在接通：正在换一条路试'),
    ).toBeOnTheScreen();
    expect(screen.queryByText(/Mac/)).toBeNull();
  });

  it('cancels the connection', async () => {
    const cancelConnect = jest.fn();
    await renderWithProviders(
      <ConnectingOverlay ctx={context({ cancelConnect })} />,
    );
    await fireEvent.press(screen.getByLabelText('取消'));
    expect(cancelConnect).toHaveBeenCalledTimes(1);
  });
});

describe('ConnectingOverlay slow first frame', () => {
  function streamingCtx(overrides: Partial<SessionCtx> = {}) {
    const base = context();
    return context({
      rec: { ...base.rec!, phase: 'streaming' },
      phaseAtMs: { authenticating: 400, streaming: 700 },
      ...overrides,
    });
  }

  it('offers retry and lower quality 15 seconds after streaming starts', async () => {
    const onRetry = jest.fn();
    const setQuality = jest.fn();
    await renderWithProviders(
      <ConnectingOverlay
        ctx={streamingCtx({ setQuality })}
        onRetry={onRetry}
      />,
    );
    await advance(15_000);
    expect(screen.queryByTestId('connect-slow')).toBeNull();
    await advance(1_000);
    expect(screen.getByText('画面一直没出来')).toBeOnTheScreen();
    expect(screen.queryByTestId('connect-stuck')).toBeNull();

    await fireEvent.press(screen.getByTestId('connect-slow-quality'));
    expect(setQuality).toHaveBeenCalledWith('eco');
    await fireEvent.press(screen.getByTestId('connect-slow-retry'));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('hides lower quality when already on the lowest quality', async () => {
    await renderWithProviders(
      <ConnectingOverlay
        ctx={streamingCtx({ quality: 'eco' })}
        onRetry={jest.fn()}
      />,
    );
    await advance(16_000);
    expect(screen.getByTestId('connect-slow')).toBeOnTheScreen();
    expect(screen.queryByTestId('connect-slow-quality')).toBeNull();
  });

  it('collapses the session while connecting', async () => {
    const collapse = jest.fn();
    await renderWithProviders(
      <ConnectingOverlay ctx={context({ collapse })} />,
    );
    await fireEvent.press(screen.getByTestId('connect-collapse'));
    expect(collapse).toHaveBeenCalledTimes(1);
  });
});

describe('connectPath', () => {
  it('falls back to the directory tailnet address', () => {
    expect(connectPath({ dial: [], host: computers[0] })).toEqual({
      path: 'tailnetDirect',
      node: 'studio-mac',
      addr: '100.64.0.7',
    });
  });

  it('ignores failed attempts', () => {
    expect(
      connectPath({
        dial: [
          { path: 'lan', addr: '192.168.31.20', state: 'failed' },
          { path: 'tailnetDerp', addr: '100.64.0.7', state: 'skipped' },
        ],
        host: computers[0],
      }),
    ).toEqual({
      path: 'tailnetDirect',
      node: 'studio-mac',
      addr: '100.64.0.7',
    });
  });
});
