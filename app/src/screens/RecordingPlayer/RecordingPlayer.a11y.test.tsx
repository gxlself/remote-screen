import { act, cleanup, screen, waitFor } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { RemoteSessionViewProps } from '../../native/session/RemoteSessionView';
import { expectLabels, swallowedControls } from '../../test/a11y/helpers';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import RecordingPlayerScreen from './index';

jest.setTimeout(30000);

jest.mock('../../native/audio/audio', () => ({
  startAudio: jest.fn(async () => true),
  stopAudio: jest.fn(),
  setAudioMuted: jest.fn(),
}));

jest.mock('../../native/session/RemoteSessionView', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function RemoteSessionView(props: RemoteSessionViewProps) {
    return (
      <MockView
        accessible
        accessibilityLabel={props.accessibilityLabel}
        accessibilityRole="image"
        testID={`player-surface-${props.sessionId ?? 'none'}`}
      />
    );
  }
  return { __esModule: true, RemoteSessionView, default: RemoteSessionView };
});

const PLAYBACK_ID = '4611686018427387905';

async function mount() {
  const core = createFakeCore();
  (core as RsCoreApi).recording = {
    start: jest.fn(),
    stop: jest.fn(),
    status: jest.fn(),
    list: jest.fn(async () => ({
      recordings: [],
      storage: { usedBytes: 0, capBytes: 1 },
    })),
    remove: jest.fn(),
    open: jest.fn(async () => ({
      playbackId: PLAYBACK_ID,
      recordingId: 'r1',
      durationMs: 120000,
      width: 1920,
      height: 1080,
      hasAudio: true,
    })),
    control: jest.fn(),
    close: jest.fn(),
    exportMp4: jest.fn(),
    discardExport: jest.fn(),
  };
  const view = await renderWithProviders(
    <RecordingPlayerScreen
      navigation={mockNavigation<'RecordingPlayer'>()}
      route={mockRoute('RecordingPlayer', { recordingId: 'r1' })}
    />,
    { core },
  );
  await waitFor(() =>
    expect(screen.getByTestId(`player-surface-${PLAYBACK_ID}`)).toBeTruthy(),
  );
  return { view, core };
}

afterEach(async () => {
  await cleanup();
});

it('names the back, play / pause and export buttons in Chinese', async () => {
  const { view, core } = await mount();

  for (const name of ['返回', '播放录制', '导出录制视频']) {
    const button = screen.getByRole('button', { name });
    expect(button).toHaveProp('accessibilityRole', 'button');
    expect(button).toHaveProp('accessibilityLabel', name);
  }
  await act(() =>
    core.emit({
      type: 'playback.state',
      playbackId: PLAYBACK_ID,
      recordingId: 'r1',
      state: 'playing',
      positionMs: 30000,
      durationMs: 120000,
    }),
  );
  expect(screen.getByRole('button', { name: '暂停录制' })).toHaveProp(
    'accessibilityRole',
    'button',
  );
  expect(screen.getByRole('image', { name: '录制画面' })).toBeOnTheScreen();
  expectLabels(view.root);
  expect(swallowedControls(view.root)).toEqual([]);
});

it('exposes the seek bar as adjustable with its value', async () => {
  const { core } = await mount();
  const bar = screen.getByRole('adjustable', { name: '拖动播放位置' });

  expect(bar).toHaveProp('accessibilityValue', {
    min: 0,
    max: 120000,
    now: 0,
    text: '0:00 / 2:00',
  });
  expect(bar).toHaveProp('accessibilityActions', [
    { name: 'increment' },
    { name: 'decrement' },
  ]);
  await act(() =>
    core.emit({
      type: 'playback.state',
      playbackId: PLAYBACK_ID,
      recordingId: 'r1',
      state: 'paused',
      positionMs: 45000,
      durationMs: 120000,
    }),
  );
  expect(screen.getByTestId('player-seekbar')).toHaveProp(
    'accessibilityValue',
    { min: 0, max: 120000, now: 45000, text: '0:45 / 2:00' },
  );
});
