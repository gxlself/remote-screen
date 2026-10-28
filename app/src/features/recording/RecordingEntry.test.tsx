import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { Computer, RecordingStatus } from '../../native/rscore/types';
import type { SessionController } from '../../screens/Session/useSessionController';
import type { SessionCtx } from '../../screens/Session/slots';
import { recording } from '../../i18n/zh/recording';
import RecordingChip from './RecordingChip';
import { useRecordingMoreRow } from './RecordingMoreRow';
import { useSessionRecording } from './useSessionRecording';
import { resetRecordingUiForTests } from './sessionRecording';

// eslint-disable-next-line no-bitwise
jest.mock('./noticeBit', () => ({ RECORDING_NOTICE_BIT: 1 << 30 }));

const SESSION_ID = 's1';
// eslint-disable-next-line no-bitwise
const TEST_BIT = 1 << 30;

type TestRecordingApi = NonNullable<RsCoreApi['recording']> & {
  start: jest.Mock;
  stop: jest.Mock;
  status: jest.Mock;
};

function host(features = TEST_BIT): Computer {
  return {
    hostId: 'h1',
    name: 'Host',
    node: 'host',
    os: 'macos',
    online: true,
    hostRunning: true,
    lastSeenAt: 0,
    lastHeartbeatAt: null,
    hostVersion: '1.0.0',
    features,
    fp: 'fp',
    addresses: [],
    displays: [],
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: '',
    filevault: false,
    console: null,
  };
}

function status(
  state: RecordingStatus['state'] = 'idle',
  overrides: Partial<RecordingStatus> = {},
): RecordingStatus {
  return {
    sessionId: SESSION_ID,
    state,
    bytes: 0,
    durationMs: 0,
    ...overrides,
  };
}

function recordingApi(initial: RecordingStatus = status()): TestRecordingApi {
  return {
    start: jest.fn(async (sessionId: string) =>
      status('recording', {
        sessionId,
        recordingId: 'r1',
      }),
    ),
    stop: jest.fn(),
    status: jest.fn(() => initial),
    list: jest.fn(async () => ({
      recordings: [],
      storage: { usedBytes: 0, capBytes: 1 },
    })),
    remove: jest.fn(async () => undefined),
    open: jest.fn(async () => ({
      playbackId: 'p1',
      recordingId: 'r1',
      durationMs: 0,
      width: 1,
      height: 1,
      hasAudio: false,
    })),
    control: jest.fn(),
    close: jest.fn(),
    exportMp4: jest.fn(async () => ({ path: '', bytes: 0, mime: 'video/mp4' })),
    discardExport: jest.fn(),
  } as TestRecordingApi;
}

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: 'h1',
    hostName: 'Host',
    os: 'macos',
    host: host(),
    myDeviceId: null,
    sessionId: SESSION_ID,
    videoRef: { current: null },
    rec: { phase: 'streaming' } as SessionCtx['rec'],
    view: {} as SessionCtx['view'],
    hud: {} as SessionCtx['hud'],
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
    disconnect: jest.fn(),
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
    ...overrides,
  };
}

function Entry({ ctx, onClose }: { ctx: SessionCtx; onClose: () => void }) {
  useSessionRecording(ctx as unknown as SessionController);
  const row = useRecordingMoreRow(ctx, onClose);
  return (
    <>
      {row}
      <RecordingChip ctx={ctx} />
    </>
  );
}

async function renderEntry(
  api: TestRecordingApi | undefined,
  opts: { introShown?: boolean; ctx?: Partial<SessionCtx> } = {},
) {
  const core = createFakeCore();
  if (api !== undefined) {
    Object.assign(core, { recording: api });
  }
  const sessionCtx = context(opts.ctx);
  return renderWithProviders(<Entry ctx={sessionCtx} onClose={jest.fn()} />, {
    core,
    settings: { 'recording.introShown': opts.introShown ?? true },
  });
}

beforeEach(() => {
  resetRecordingUiForTests();
});

afterEach(async () => {
  await cleanup();
});

it('starts from the more row and shows the chip', async () => {
  const api = recordingApi();
  const onClose = jest.fn();
  const core = createFakeCore();
  Object.assign(core, { recording: api });
  const calls: string[] = [];
  onClose.mockImplementation(() => calls.push('close'));
  api.start.mockImplementation(async (sessionId: string) => {
    calls.push('start');
    return status('recording', { sessionId, recordingId: 'r1' });
  });
  await renderWithProviders(<Entry ctx={context()} onClose={onClose} />, {
    core,
    settings: { 'recording.introShown': true },
  });

  await act(async () => {
    fireEvent.press(screen.getByTestId('more-recording-start'));
    await Promise.resolve();
  });
  expect(calls).toEqual(['close', 'start']);
  expect(await screen.findByTestId('recording-chip')).toBeOnTheScreen();
  expect(screen.getByText('录制中 0:00')).toBeOnTheScreen();
});

it('shows the first-use confirmation and lets the user cancel', async () => {
  const api = recordingApi();
  await renderEntry(api, { introShown: false });

  await act(() => {
    fireEvent.press(screen.getByTestId('more-recording-start'));
  });
  expect(await screen.findByTestId('recording-confirm')).toBeOnTheScreen();
  expect(await screen.findByText(recording.introBodyA)).toBeOnTheScreen();
  fireEvent.press(screen.getByText(recording.cancel));
  await waitFor(() =>
    expect(screen.queryByTestId('recording-confirm')).toBeNull(),
  );
  expect(api.start).not.toHaveBeenCalled();
});

it('uses the old-host confirmation when the feature bit is absent', async () => {
  const api = recordingApi();
  await renderEntry(api, { ctx: { host: host(0) }, introShown: true });

  await act(() => {
    fireEvent.press(screen.getByTestId('more-recording-start'));
  });
  expect(
    await screen.findByText(recording.hostWontKnowTitle),
  ).toBeOnTheScreen();
  expect(await screen.findByText(recording.hostWontKnowBody)).toBeOnTheScreen();
});

it('updates the chip and toasts a stop reason from events', async () => {
  const api = recordingApi();
  const core = createFakeCore();
  Object.assign(core, { recording: api });
  const ctx = context();
  await renderWithProviders(<Entry ctx={ctx} onClose={jest.fn()} />, {
    core,
    settings: { 'recording.introShown': true },
  });
  await act(() =>
    core.emit({
      type: 'recording.state',
      sessionId: SESSION_ID,
      recordingId: 'r1',
      state: 'recording',
      durationMs: 65_000,
      bytes: 10,
    }),
  );
  expect(screen.getByText('录制中 1:05')).toBeOnTheScreen();
  await act(() =>
    core.emit({
      type: 'recording.state',
      sessionId: SESSION_ID,
      recordingId: 'r1',
      state: 'stopped',
      durationMs: 65_000,
      bytes: 10,
      reason: 'storageFull',
    }),
  );
  expect(screen.queryByTestId('recording-chip')).toBeNull();
  expect(ctx.showToast).toHaveBeenCalledWith(recording.stopStorageFull, {
    tone: 'good',
  });
});

it('stops from the more row and disables it while stopping', async () => {
  const api = recordingApi(status('recording', { recordingId: 'r1' }));
  await renderEntry(api);
  expect(screen.getByTestId('more-recording-stop')).toBeOnTheScreen();
  await act(() => {
    fireEvent.press(screen.getByTestId('more-recording-stop'));
  });
  expect(api.stop).toHaveBeenCalledWith(SESSION_ID);
  await waitFor(() =>
    expect(screen.getByTestId('more-recording-stop')).toHaveProp(
      'accessibilityState',
      { disabled: true },
    ),
  );
});

it('syncs an active recording from status on mount', async () => {
  const api = recordingApi(
    status('recording', { recordingId: 'r1', durationMs: 3_000 }),
  );
  await renderEntry(api);
  expect(screen.getByText('录制中 0:03')).toBeOnTheScreen();
  expect(api.status).toHaveBeenCalledWith(SESSION_ID);
});

it('is inert for old cores and non-streaming sessions', async () => {
  await renderEntry(undefined);
  expect(screen.queryByTestId('more-recording-start')).toBeNull();
  expect(screen.queryByTestId('recording-chip')).toBeNull();
  await cleanup();
  resetRecordingUiForTests();
  await renderEntry(recordingApi(), {
    ctx: { rec: { phase: 'connecting' } as SessionCtx['rec'] },
  });
  expect(screen.queryByTestId('more-recording-start')).toBeNull();
});

it('stops from the chip only after confirming', async () => {
  const api = recordingApi(
    status('recording', { recordingId: 'r1', durationMs: 5_000 }),
  );
  await renderEntry(api);
  await act(() => {
    fireEvent.press(screen.getByTestId('recording-chip'));
  });
  expect(await screen.findByText(recording.stopConfirmTitle)).toBeOnTheScreen();
  expect(api.stop).not.toHaveBeenCalled();

  fireEvent.press(
    within(screen.getByTestId('recording-stop-confirm-actions')).getByText(
      recording.cancel,
    ),
  );
  await waitFor(() =>
    expect(screen.queryByText(recording.stopConfirmTitle)).toBeNull(),
  );
  expect(api.stop).not.toHaveBeenCalled();

  await act(() => {
    fireEvent.press(screen.getByTestId('recording-chip'));
  });
  await act(() => {
    fireEvent.press(
      within(screen.getByTestId('recording-stop-confirm-actions')).getByText(
        recording.stopConfirm,
      ),
    );
  });
  expect(api.stop).toHaveBeenCalledWith(SESSION_ID);
  expect(
    within(screen.getByTestId('recording-chip')).getByText(
      recording.chipStopping,
    ),
  ).toBeOnTheScreen();
  expect(screen.getByTestId('recording-chip')).toBeDisabled();
});

it('shows starting and stopping states on the chip and the more row', async () => {
  const api = recordingApi();
  let finish: (value: RecordingStatus) => void = () => undefined;
  api.start.mockImplementation(
    () =>
      new Promise<RecordingStatus>(resolve => {
        finish = resolve;
      }),
  );
  await renderEntry(api);
  await act(async () => {
    fireEvent.press(screen.getByTestId('more-recording-start'));
    await Promise.resolve();
  });
  expect(
    within(screen.getByTestId('recording-chip')).getByText(
      recording.chipStarting,
    ),
  ).toBeOnTheScreen();
  expect(screen.getByTestId('more-recording-starting')).toBeDisabled();

  await act(async () => {
    finish(status('recording', { recordingId: 'r1' }));
    await Promise.resolve();
  });
  expect(screen.getByText('录制中 0:00')).toBeOnTheScreen();
});

it('reports a stop that could not be sent', async () => {
  const api = recordingApi(status('recording', { recordingId: 'r1' }));
  api.stop.mockImplementation(() => {
    throw new Error('closed');
  });
  const ctx = context();
  const core = createFakeCore();
  Object.assign(core, { recording: api });
  await renderWithProviders(<Entry ctx={ctx} onClose={jest.fn()} />, {
    core,
    settings: { 'recording.introShown': true },
  });
  await act(() => {
    fireEvent.press(screen.getByTestId('more-recording-stop'));
  });
  expect(ctx.showToast).toHaveBeenCalledWith(recording.stopFailed, {
    tone: 'bad',
  });
});
