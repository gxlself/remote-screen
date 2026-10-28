import { act, fireEvent, screen } from '@testing-library/react-native';
import { computers, liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { ApprovalOverlay } from './ApprovalOverlay';
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
      phase: 'pendingApproval',
      approvalTimeoutS: 30,
      controller: null,
      viewers: [],
      inputPausedLocal: false,
      stats: null,
      displays: [],
      dial: [],
      derpRegion: null,
      reconnectReason: null,
      idleWarning: null,
      controlRequest: null,
      lastError: null,
      clipboard: [],
    },
    view: {
      overlay: 'approval',
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

it('shows the approval title and ticks down without ending the session', async () => {
  const ctx = context();
  const cancelConnect = jest.fn();
  const disconnect = jest.fn();
  await renderWithProviders(
    <ApprovalOverlay ctx={{ ...ctx, cancelConnect, disconnect }} />,
  );

  expect(screen.getByText('等待 Mac 上允许')).toBeOnTheScreen();
  expect(screen.getByText('30')).toBeOnTheScreen();
  await advance(5000);
  expect(screen.getByText('25')).toBeOnTheScreen();
  await advance(26000);
  expect(cancelConnect).not.toHaveBeenCalled();
  expect(disconnect).not.toHaveBeenCalled();
});

it('cancels the pending approval', async () => {
  const cancelConnect = jest.fn();
  await renderWithProviders(
    <ApprovalOverlay ctx={context({ cancelConnect })} />,
  );
  await fireEvent.press(screen.getByLabelText('取消'));
  expect(cancelConnect).toHaveBeenCalledTimes(1);
});

it('uses the Windows wording for the approval title', async () => {
  await renderWithProviders(
    <ApprovalOverlay ctx={context({ os: 'windows' })} />,
  );
  expect(screen.getByText('等待电脑上允许')).toBeOnTheScreen();
});
