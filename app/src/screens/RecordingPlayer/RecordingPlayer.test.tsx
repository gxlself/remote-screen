import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { exportRecording } from '../../features/recording/exportShare';
import { startAudio, stopAudio } from '../../native/audio/audio';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type {
  CoreEventOf,
  PlaybackInfo,
  RecordingInfo,
  RecordingList,
} from '../../native/rscore/types';
import type { RemoteSessionViewProps } from '../../native/session/RemoteSessionView';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import RecordingPlayerScreen from './index';

jest.setTimeout(30000);

jest.mock('../../native/audio/audio', () => ({
  startAudio: jest.fn(async () => true),
  stopAudio: jest.fn(),
  setAudioMuted: jest.fn(),
}));
jest.mock('../../features/recording/exportShare', () => ({
  exportRecording: jest.fn(async () => 'unsupported'),
}));

const mockViewProps = jest.fn<void, [RemoteSessionViewProps]>();
jest.mock('../../native/session/RemoteSessionView', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function RemoteSessionView(props: RemoteSessionViewProps) {
    mockViewProps(props);
    return (
      <MockView
        accessible
        accessibilityLabel={props.accessibilityLabel}
        accessibilityRole="image"
        testID="player-surface"
      />
    );
  }
  return { __esModule: true, RemoteSessionView, default: RemoteSessionView };
});

const PLAYBACK_ID = '4611686018427387905';

type RecordingApi = NonNullable<RsCoreApi['recording']>;
type FakeRecording = { [K in keyof RecordingApi]: jest.Mock };

function info(overrides: Partial<PlaybackInfo> = {}): PlaybackInfo {
  return {
    playbackId: PLAYBACK_ID,
    recordingId: 'r1',
    durationMs: 120000,
    width: 1920,
    height: 1080,
    hasAudio: true,
    ...overrides,
  };
}

function listed(overrides: Partial<RecordingInfo> = {}): RecordingInfo {
  return {
    recordingId: 'r1',
    hostId: 'host-1',
    hostName: 'Studio Mac mini',
    startedAt: Date.now() * 1000,
    durationMs: 120000,
    bytes: 4096,
    state: 'complete',
    hasAudio: true,
    width: 1920,
    height: 1080,
    ...overrides,
  };
}

function fakeRecording(
  opts: {
    open?: () => Promise<PlaybackInfo>;
    list?: RecordingList;
  } = {},
): FakeRecording {
  return {
    start: jest.fn(),
    stop: jest.fn(),
    status: jest.fn(),
    list: jest.fn(
      async () =>
        opts.list ?? {
          recordings: [],
          storage: { usedBytes: 0, capBytes: 1 },
        },
    ),
    remove: jest.fn(async () => undefined),
    open: jest.fn(opts.open ?? (async () => info())),
    control: jest.fn(),
    close: jest.fn(),
    exportMp4: jest.fn(),
    discardExport: jest.fn(),
  };
}

async function mount(fake: FakeRecording | undefined) {
  const core = createFakeCore();
  if (fake !== undefined) {
    (core as RsCoreApi).recording = fake as unknown as RecordingApi;
  }
  const navigation = mockNavigation<'RecordingPlayer'>();
  const view = await renderWithProviders(
    <RecordingPlayerScreen
      navigation={navigation}
      route={mockRoute('RecordingPlayer', { recordingId: 'r1' })}
    />,
    { core },
  );
  return { view, core, navigation };
}

async function mountOpened(fake = fakeRecording()) {
  const mounted = await mount(fake);
  await waitFor(() =>
    expect(mockViewProps).toHaveBeenLastCalledWith(
      expect.objectContaining({ sessionId: PLAYBACK_ID }),
    ),
  );
  return { ...mounted, fake };
}

function playbackEvent(
  patch: Partial<CoreEventOf<'playback.state'>>,
): CoreEventOf<'playback.state'> {
  return {
    type: 'playback.state',
    playbackId: PLAYBACK_ID,
    recordingId: 'r1',
    state: 'playing',
    positionMs: 0,
    durationMs: 120000,
    ...patch,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(next => {
    resolve = next;
  });
  return { promise, resolve };
}

beforeEach(() => {
  jest.clearAllMocks();
  jest.mocked(startAudio).mockResolvedValue(true);
  jest.mocked(exportRecording).mockResolvedValue('unsupported');
});

afterEach(async () => {
  await cleanup();
});

it('opens the recording, attaches the playback id and closes on unmount', async () => {
  const fake = fakeRecording();
  const { view } = await mountOpened(fake);

  expect(fake.open).toHaveBeenCalledWith('r1');
  expect(mockViewProps).toHaveBeenLastCalledWith(
    expect.objectContaining({
      sessionId: PLAYBACK_ID,
      mode: 'touch',
      toolbarCollapsed: false,
      keyboardInset: 0,
      accessibilityLabel: '录制画面',
    }),
  );
  expect(mockViewProps.mock.lastCall?.[0].onGesture).toBeUndefined();
  expect(startAudio).toHaveBeenCalledWith(PLAYBACK_ID);

  await view.unmount();
  expect(fake.close).toHaveBeenCalledWith(PLAYBACK_ID);
  expect(stopAudio).toHaveBeenCalledTimes(1);
});

it('follows playback.state for its own playback id only', async () => {
  const { core, fake } = await mountOpened();

  await act(() =>
    core.emit(playbackEvent({ state: 'playing', positionMs: 30000 })),
  );
  expect(screen.getByText('0:30 / 2:00')).toBeOnTheScreen();
  expect(screen.getByTestId('player-seekbar-played')).toHaveStyle({
    width: '25%',
  });
  await fireEvent.press(screen.getByRole('button', { name: '暂停录制' }));
  expect(fake.control).toHaveBeenCalledWith(PLAYBACK_ID, { op: 'pause' });
  expect(screen.getByRole('button', { name: '播放录制' })).toBeOnTheScreen();

  await act(() =>
    core.emit(
      playbackEvent({
        playbackId: '4611686018427387906',
        state: 'playing',
        positionMs: 90000,
      }),
    ),
  );
  expect(screen.getByText('0:30 / 2:00')).toBeOnTheScreen();
  expect(screen.getByRole('button', { name: '播放录制' })).toBeOnTheScreen();
});

it('seeks from the tapped position and from accessibility actions', async () => {
  const { fake } = await mountOpened();
  const bar = screen.getByTestId('player-seekbar');

  await fireEvent(bar, 'layout', {
    nativeEvent: { layout: { x: 0, y: 0, width: 200, height: 44 } },
  });
  await fireEvent.press(bar, { nativeEvent: { locationX: 50 } });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 30000,
  });
  expect(screen.getByText('0:30 / 2:00')).toBeOnTheScreen();

  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'increment' },
  });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 40000,
  });

  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'decrement' },
  });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 30000,
  });
});

it('clamps seeks to the recording and sends whole milliseconds', async () => {
  const { fake } = await mountOpened();
  const bar = screen.getByTestId('player-seekbar');

  await fireEvent(bar, 'layout', {
    nativeEvent: { layout: { x: 0, y: 0, width: 350, height: 44 } },
  });
  await fireEvent.press(bar, { nativeEvent: { locationX: 100 } });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 34286,
  });
  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'decrement' },
  });
  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'decrement' },
  });
  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'decrement' },
  });
  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'decrement' },
  });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 0,
  });
  await fireEvent.press(bar, { nativeEvent: { locationX: 400 } });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 120000,
  });
  await fireEvent(bar, 'accessibilityAction', {
    nativeEvent: { actionName: 'increment' },
  });
  expect(fake.control).toHaveBeenLastCalledWith(PLAYBACK_ID, {
    op: 'seek',
    positionMs: 120000,
  });
});

it('shows play after the recording ends and restarts on press', async () => {
  const { core, fake } = await mountOpened();

  await act(() =>
    core.emit(playbackEvent({ state: 'ended', positionMs: 120000 })),
  );
  expect(screen.getByText('2:00 / 2:00')).toBeOnTheScreen();
  expect(screen.queryByRole('button', { name: '暂停录制' })).toBeNull();
  await fireEvent.press(screen.getByRole('button', { name: '播放录制' }));
  expect(fake.control).toHaveBeenCalledWith(PLAYBACK_ID, { op: 'play' });
  expect(screen.getByRole('button', { name: '暂停录制' })).toBeOnTheScreen();
});

it('shows the load failure for a damaged file without starting audio', async () => {
  const fake = fakeRecording({
    open: async () => {
      throw new RsCoreError('http', 'recording.damaged', 422);
    },
  });
  const { navigation } = await mount(fake);

  expect(await screen.findByText('加载录制失败')).toBeOnTheScreen();
  expect(
    screen.getByText('这份录制有损坏，只能播放到损坏之前的部分'),
  ).toBeOnTheScreen();
  expect(startAudio).not.toHaveBeenCalled();
  expect(screen.queryByTestId('player-controls')).toBeNull();
  await fireEvent.press(screen.getByRole('button', { name: '返回' }));
  expect(navigation.goBack).toHaveBeenCalled();
});

it('shows an error state when the core has no recording namespace', async () => {
  await mount(undefined);

  expect(await screen.findByText('当前版本不支持会话录制')).toBeOnTheScreen();
  expect(screen.getByTestId('player-error')).toBeOnTheScreen();
  expect(startAudio).not.toHaveBeenCalled();
});

it('shows a playback error reported by the event stream', async () => {
  const { core } = await mountOpened();

  await act(() =>
    core.emit(
      playbackEvent({ state: 'error', error: 'recording.notFound: r1' }),
    ),
  );
  expect(screen.getByText('加载录制失败')).toBeOnTheScreen();
  expect(screen.getByTestId('player-error')).toBeOnTheScreen();
});

it('closes a playback that opens after the page has left', async () => {
  const pending = deferred<PlaybackInfo>();
  const fake = fakeRecording({ open: () => pending.promise });
  const { view } = await mount(fake);
  expect(fake.open).toHaveBeenCalledWith('r1');

  await view.unmount();
  expect(fake.close).not.toHaveBeenCalled();
  await act(async () => {
    pending.resolve(info());
    await pending.promise;
  });
  expect(fake.close).toHaveBeenCalledWith(PLAYBACK_ID);
  expect(startAudio).not.toHaveBeenCalled();
  expect(stopAudio).not.toHaveBeenCalled();
});

it('applies a state event that arrives before open returns', async () => {
  const pending = deferred<PlaybackInfo>();
  const fake = fakeRecording({ open: () => pending.promise });
  const { core } = await mount(fake);

  await act(() =>
    core.emit(playbackEvent({ state: 'paused', positionMs: 5000 })),
  );
  expect(screen.getByTestId('player-loading')).toBeOnTheScreen();
  await act(async () => {
    pending.resolve(info());
    await pending.promise;
  });
  expect(screen.queryByTestId('player-loading')).toBeNull();
  expect(screen.getByText('0:05 / 2:00')).toBeOnTheScreen();
});

it('pauses, then reports a saved export with the no-audio note', async () => {
  const { fake } = await mountOpened();
  jest.mocked(exportRecording).mockResolvedValue('saved');

  await fireEvent.press(screen.getByRole('button', { name: '导出录制视频' }));
  expect(
    await screen.findByText('已保存，导出的视频不含声音'),
  ).toBeOnTheScreen();
  expect(fake.control).toHaveBeenCalledWith(PLAYBACK_ID, { op: 'pause' });
  expect(exportRecording).toHaveBeenCalledWith(expect.anything(), 'r1');
  expect(fake.control.mock.invocationCallOrder[0]).toBeLessThan(
    jest.mocked(exportRecording).mock.invocationCallOrder[0],
  );
});

it('reports unsupported, busy and failed exports', async () => {
  await mountOpened();
  const button = () => screen.getByRole('button', { name: '导出录制视频' });

  await fireEvent.press(button());
  expect(await screen.findByText('当前版本不支持导出')).toBeOnTheScreen();

  jest
    .mocked(exportRecording)
    .mockRejectedValueOnce(new RsCoreError('http', 'recording.busy', 429));
  await fireEvent.press(button());
  expect(await screen.findByText('正在导出另一份录制')).toBeOnTheScreen();

  jest
    .mocked(exportRecording)
    .mockRejectedValueOnce(new RsCoreError('internal', 'recording.io'));
  await fireEvent.press(button());
  expect(await screen.findByText('导出失败')).toBeOnTheScreen();
});

it('stays quiet when the export is cancelled', async () => {
  await mountOpened();
  jest.mocked(exportRecording).mockResolvedValue('cancelled');

  await fireEvent.press(screen.getByRole('button', { name: '导出录制视频' }));
  await act(() => Promise.resolve());
  expect(exportRecording).toHaveBeenCalledTimes(1);
  expect(
    screen.getByRole('button', { name: '导出录制视频' }),
  ).not.toBeDisabled();
  expect(screen.queryByTestId('toast')).toBeNull();
});

it('disables export while it is running', async () => {
  await mountOpened();
  const pending = deferred<'saved' | 'cancelled' | 'unsupported'>();
  jest.mocked(exportRecording).mockReturnValue(pending.promise);

  await fireEvent.press(screen.getByRole('button', { name: '导出录制视频' }));
  expect(screen.getByRole('button', { name: '导出录制视频' })).toBeDisabled();
  expect(screen.getByText('正在导出…')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '导出录制视频' }));
  expect(exportRecording).toHaveBeenCalledTimes(1);

  await act(async () => {
    pending.resolve('cancelled');
    await pending.promise;
  });
  expect(
    screen.getByRole('button', { name: '导出录制视频' }),
  ).not.toBeDisabled();
  expect(screen.queryByText('正在导出…')).toBeNull();
});

it('toasts when the core rejects a playback command', async () => {
  const fake = fakeRecording();
  fake.control.mockImplementation(() => {
    throw new RsCoreError('internal', 'recording.badCommand');
  });
  await mountOpened(fake);

  await fireEvent.press(screen.getByRole('button', { name: '播放录制' }));
  expect(screen.getByTestId('toast')).toHaveProp(
    'accessibilityLabel',
    '加载录制失败',
  );
  expect(screen.getByRole('button', { name: '播放录制' })).toBeOnTheScreen();
});

it('titles the page from the cached row and notes damaged recordings', async () => {
  await mountOpened(
    fakeRecording({
      list: {
        recordings: [listed({ state: 'damaged' })],
        storage: { usedBytes: 4096, capBytes: 2 ** 30 },
      },
    }),
  );

  expect(await screen.findByText('Studio Mac mini')).toBeOnTheScreen();
  expect(screen.getByTestId('player-damaged')).toHaveProp(
    'accessibilityLabel',
    '这份录制有损坏，只能播放到损坏之前的部分',
  );
  expect(
    screen.getByRole('header', { name: 'Studio Mac mini' }),
  ).toBeOnTheScreen();
});

it('falls back to the section title when the row is not cached', async () => {
  await mountOpened();

  expect(screen.getByRole('header', { name: '会话录制' })).toBeOnTheScreen();
  expect(screen.queryByTestId('player-damaged')).toBeNull();
});

it('leaves audio alone for a recording without sound', async () => {
  const fake = fakeRecording({ open: async () => info({ hasAudio: false }) });
  const { view } = await mountOpened(fake);

  expect(startAudio).not.toHaveBeenCalled();
  await view.unmount();
  expect(fake.close).toHaveBeenCalledWith(PLAYBACK_ID);
  expect(stopAudio).not.toHaveBeenCalled();
});
