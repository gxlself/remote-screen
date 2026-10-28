import type { ReactElement } from 'react';
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react-native';
import { AppState, Platform } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { renderWithProviders } from '../../test/render';
import { mockWindowDimensions } from '../../test/dimensions';
import type { SessionRecord } from '../session/store';
import type { RemoteVideoStats } from '../../native/session/RemoteSessionView';
import type { SessionCtx } from '../../screens/Session/slots';
import MoreSheet from '../../screens/Session/MoreSheet';
import { pip } from '../../i18n/zh/pip';
import { clearVideo, setVideoStats } from '../session/videoStore';
import { applyPipChange, resetPipStateForTests } from './pipState';
import { PipPlaceholder } from './PipPlaceholder';
import { usePipMoreRow } from './PipMoreRow';

const mockPipSupported = jest.fn(() => true);
const mockStartPip = jest.fn();
const mockStopPip = jest.fn();

jest.mock('./nativePip', () => ({
  pipSupported: () => mockPipSupported(),
  startPip: () => mockStartPip(),
  stopPip: () => mockStopPip(),
}));

const SESSION_ID = 's1';

function stats(overrides: Partial<RemoteVideoStats> = {}): RemoteVideoStats {
  return {
    fps: 60,
    decodeP50Ms: 1,
    decodeP95Ms: 2,
    latencyP50Ms: 3,
    queue: 0,
    dropped: 0,
    keyframeRequests: 0,
    width: 1920,
    height: 1080,
    codec: 'h264',
    ...overrides,
  };
}

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    id: SESSION_ID,
    hostId: 'host-mac',
    hostName: 'Studio Mac mini',
    phase: 'streaming',
    role: 'viewer',
    path: 'tailnetDirect',
    transport: 'quic',
    startedAt: 1,
    displayId: 1,
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

function ctx(overrides: Partial<SessionCtx> = {}): SessionCtx {
  const rec = record();
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: SESSION_ID,
    rec,
    view: {
      overlay: 'none',
      end: { kind: 'stay' },
      role: 'viewer',
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
      a11y: '正在测量',
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

function MoreRowHarness({
  sessionCtx,
  onClose,
}: {
  sessionCtx: SessionCtx;
  onClose: () => void;
}): ReactElement | null {
  return usePipMoreRow(sessionCtx, onClose);
}

function spyOnAppState(): {
  listeners: ((state: AppStateStatus) => void)[];
  remove: jest.Mock;
} {
  const listeners: ((state: AppStateStatus) => void)[] = [];
  const remove = jest.fn();
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_type, cb) => {
    listeners.push(cb as (state: AppStateStatus) => void);
    return { remove };
  });
  return { listeners, remove };
}

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'active',
  });
  resetPipStateForTests();
  mockPipSupported.mockReturnValue(true);
  mockStartPip.mockReset();
  mockStopPip.mockReset();
  setVideoStats(SESSION_ID, stats());
});

afterEach(async () => {
  clearVideo(SESSION_ID);
  await cleanup();
  jest.restoreAllMocks();
});

it('shows the PiP row for an eligible iOS viewer', async () => {
  await render(<MoreRowHarness onClose={jest.fn()} sessionCtx={ctx()} />);

  expect(screen.getByTestId('more-pip')).toBeOnTheScreen();
  expect(screen.getByText(pip.moreRow)).toBeOnTheScreen();
  expect(screen.getByText(pip.moreRowSubtitle)).toBeOnTheScreen();
});

it('closes the sheet before starting PiP', async () => {
  const calls: string[] = [];
  const onClose = jest.fn(() => calls.push('close'));
  mockStartPip.mockImplementation(() => calls.push('start'));
  await render(<MoreRowHarness onClose={onClose} sessionCtx={ctx()} />);

  fireEvent.press(screen.getByTestId('more-pip'));

  expect(calls).toEqual(['close', 'start']);
});

it('hides the row on Android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await render(<MoreRowHarness onClose={jest.fn()} sessionCtx={ctx()} />);
  expect(screen.queryByTestId('more-pip')).toBeNull();
});

it('hides the row when the device does not support PiP', async () => {
  mockPipSupported.mockReturnValue(false);
  await render(<MoreRowHarness onClose={jest.fn()} sessionCtx={ctx()} />);
  expect(screen.queryByTestId('more-pip')).toBeNull();
});

it('hides the row for a controller', async () => {
  await render(
    <MoreRowHarness
      onClose={jest.fn()}
      sessionCtx={ctx({
        view: { ...ctx().view, role: 'controller' },
      })}
    />,
  );
  expect(screen.queryByTestId('more-pip')).toBeNull();
});

it('hides the row while PiP is already active', async () => {
  await act(() =>
    applyPipChange({ active: true, sessionId: SESSION_ID, reason: 'started' }),
  );
  await render(<MoreRowHarness onClose={jest.fn()} sessionCtx={ctx()} />);
  expect(screen.queryByTestId('more-pip')).toBeNull();
});

it('renders the placeholder only while this session is in PiP', async () => {
  const { rerender } = await render(<PipPlaceholder ctx={ctx()} />);
  expect(screen.queryByTestId('pip-placeholder')).toBeNull();

  await act(() =>
    applyPipChange({ active: true, sessionId: SESSION_ID, reason: 'started' }),
  );
  expect(screen.getByTestId('pip-placeholder')).toBeOnTheScreen();
  expect(screen.getByText(pip.placeholderTitle)).toBeOnTheScreen();
  // R21：外层不再整体 accessible，「回到全屏」按钮单独可点。
  expect(screen.getByTestId('pip-placeholder')).not.toHaveProp(
    'accessible',
    true,
  );
  expect(screen.getByTestId('pip-placeholder-title')).toHaveProp(
    'accessibilityLabel',
    pip.placeholderA11y,
  );
  const back = screen.getByRole('button', { name: pip.placeholderBack });
  await fireEvent.press(back);
  expect(mockStopPip).toHaveBeenCalledTimes(1);

  await rerender(<PipPlaceholder ctx={ctx({ sessionId: 'other' })} />);
  expect(screen.queryByTestId('pip-placeholder')).toBeNull();
});

it('hides the placeholder while the app is not active', async () => {
  const { listeners } = spyOnAppState();
  await render(<PipPlaceholder ctx={ctx()} />);
  await act(() =>
    applyPipChange({ active: true, sessionId: SESSION_ID, reason: 'started' }),
  );
  expect(screen.getByTestId('pip-placeholder')).toBeOnTheScreen();

  await act(() => listeners[0]?.('background'));
  expect(screen.queryByTestId('pip-placeholder')).toBeNull();
});

it('places the row after the gesture help row in MoreSheet', async () => {
  const restoreDimensions = mockWindowDimensions({ width: 390, height: 844 });
  try {
    const view = await renderWithProviders(
      <MoreSheet ctx={ctx()} onClose={jest.fn()} />,
    );
    const buttons = screen.getAllByRole('button');
    const gestureIndex = buttons.findIndex(button =>
      button.props.accessibilityLabel?.startsWith('手势说明'),
    );
    const pipIndex = buttons.findIndex(button =>
      button.props.accessibilityLabel?.startsWith(pip.moreRow),
    );
    expect(gestureIndex).toBeGreaterThanOrEqual(0);
    expect(pipIndex).toBe(gestureIndex + 1);
    expect(screen.getAllByTestId('list-separator').length).toBeGreaterThan(0);
    await view.unmount();
  } finally {
    restoreDimensions();
  }
});

it('does not add a row or separator when MoreSheet cannot start PiP', async () => {
  const restoreDimensions = mockWindowDimensions({ width: 390, height: 844 });
  try {
    const view = await renderWithProviders(
      <MoreSheet ctx={ctx()} onClose={jest.fn()} />,
    );
    const separatorCount = screen.getAllByTestId('list-separator').length;
    await view.unmount();

    mockPipSupported.mockReturnValue(false);
    const unavailable = await renderWithProviders(
      <MoreSheet ctx={ctx()} onClose={jest.fn()} />,
    );
    expect(screen.queryByTestId('more-pip')).toBeNull();
    expect(screen.getAllByTestId('list-separator')).toHaveLength(
      separatorCount - 1,
    );
    await unavailable.unmount();
  } finally {
    restoreDimensions();
  }
});
