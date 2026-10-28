import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { RecordingList } from '../../native/rscore/types';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import type { RootScreenProps } from '../../app/navigation/routes';
import RecordingsScreen from './index';

jest.setTimeout(30000);

type RecordingApi = NonNullable<RsCoreApi['recording']>;
type RecordingCore = ReturnType<typeof createFakeCore> & {
  recording: RecordingApi;
};

function screenProps() {
  return {
    navigation: mockNavigation<'Recordings'>(),
    route: mockRoute('Recordings'),
  } as RootScreenProps<'Recordings'>;
}

function withRecording(list: () => Promise<RecordingList>): RecordingCore {
  const core = createFakeCore();
  const recordingCore = core as RecordingCore;
  recordingCore.recording = {
    start: jest.fn(),
    stop: jest.fn(),
    status: jest.fn(),
    list,
    remove: jest.fn(async () => undefined),
    open: jest.fn(),
    control: jest.fn(),
    close: jest.fn(),
    exportMp4: jest.fn(),
    discardExport: jest.fn(),
  } as RecordingApi;
  return recordingCore;
}

const sample: RecordingList = {
  recordings: [
    {
      recordingId: 'complete-1',
      hostId: 'host-1',
      hostName: 'Studio Mac mini',
      startedAt: Date.now() * 1000,
      durationMs: 5_000,
      bytes: 1536,
      state: 'complete',
      hasAudio: true,
      width: 1920,
      height: 1080,
    },
    {
      recordingId: 'damaged-1',
      hostId: 'host-2',
      hostName: '工位 Windows',
      startedAt: Date.now() * 1000,
      durationMs: 754_000,
      bytes: 2 ** 20,
      state: 'damaged',
      hasAudio: false,
      width: 1280,
      height: 720,
    },
    {
      recordingId: 'recording-1',
      hostId: 'host-3',
      hostName: 'MacBook',
      startedAt: Date.now() * 1000,
      durationMs: 1_000,
      bytes: 100,
      state: 'recording',
      hasAudio: true,
      width: 1280,
      height: 720,
    },
  ],
  storage: { usedBytes: 1, capBytes: 100 },
};

afterEach(async () => {
  await cleanup();
});

it('renders rows, badges, navigation, and the recording guard', async () => {
  const navigation = mockNavigation<'Recordings'>();
  const core = withRecording(jest.fn(async () => sample));
  await renderWithProviders(
    <RecordingsScreen
      navigation={navigation}
      route={mockRoute('Recordings')}
    />,
    { core },
  );

  expect(
    await screen.findByTestId('recording-row-complete-1'),
  ).toBeOnTheScreen();
  expect(screen.getAllByTestId(/recording-row-/)).toHaveLength(3);
  expect(screen.getByText('已损坏')).toBeOnTheScreen();
  expect(screen.queryByTestId('recording-delete-recording-1')).toBeNull();
  await fireEvent.press(screen.getByTestId('recording-row-complete-1'));
  expect(navigation.navigate).toHaveBeenCalledWith('RecordingPlayer', {
    recordingId: 'complete-1',
  });
});

it('confirms deletes and leaves cancellation alone', async () => {
  const remove = jest.fn(async () => undefined);
  const list = jest.fn(async () => sample);
  const core = withRecording(list);
  core.recording!.remove = remove;
  const { navigation } = screenProps();
  await renderWithProviders(
    <RecordingsScreen
      navigation={navigation}
      route={mockRoute('Recordings')}
    />,
    { core },
  );
  await screen.findByTestId('recording-row-complete-1');
  await fireEvent.press(screen.getByTestId('recording-delete-complete-1'));
  expect(screen.getByText('删除这份录制？')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '取消' }));
  expect(remove).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByTestId('recording-delete-complete-1'));
  await fireEvent.press(screen.getByRole('button', { name: '删除' }));
  await waitFor(() => expect(remove).toHaveBeenCalledWith('complete-1'));
  expect(list).toHaveBeenCalledTimes(2);
});

it('renders empty and error states', async () => {
  const empty = withRecording(
    jest.fn(async () => ({ ...sample, recordings: [] })),
  );
  await renderWithProviders(<RecordingsScreen {...screenProps()} />, {
    core: empty,
  });
  expect(await screen.findByText('还没有会话录制')).toBeOnTheScreen();
  await cleanup();

  const unsupported = withRecording(
    jest.fn(async () => {
      throw new RsCoreError('unsupported', 'recording.unsupported');
    }),
  );
  await renderWithProviders(<RecordingsScreen {...screenProps()} />, {
    core: unsupported,
  });
  expect(await screen.findByText('当前版本不支持会话录制')).toBeOnTheScreen();
});

it('refreshes after a stopped recording event', async () => {
  const list = jest.fn(async () => sample);
  const core = withRecording(list);
  await renderWithProviders(<RecordingsScreen {...screenProps()} />, { core });
  await screen.findByTestId('recording-row-complete-1');
  const before = list.mock.calls.length;
  await act(async () => {
    core.emit({
      type: 'recording.state',
      sessionId: 'session-1',
      recordingId: 'complete-1',
      state: 'stopped',
      bytes: 1,
      durationMs: 1,
    });
    await Promise.resolve();
  });
  await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(before));
});
