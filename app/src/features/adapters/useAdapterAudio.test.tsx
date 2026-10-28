import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { setAudioMuted, startAudio, stopAudio } from '../../native/audio/audio';
import { CoreProvider } from '../../native/rscore/CoreContext';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import type {
  AdapterPhase,
  AdapterProtocol,
  SessionId,
} from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { MemoryKv } from '../storage/kv';
import { createSettingsStore, SettingsProvider } from '../settings/store';
import AdapterSessionScreen from './AdapterSessionScreen';
import { installFakeAdapters } from './testing/fakeAdapters';
import { useAdapterAudio } from './useAdapterAudio';

jest.mock('../../native/audio/audio', () => ({
  startAudio: jest.fn().mockResolvedValue(true),
  stopAudio: jest.fn(),
  setAudioMuted: jest.fn(),
}));

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

const ID = '4611686018427387905';
const clients: QueryClient[] = [];

type Props = {
  sessionId: SessionId | null;
  protocol: AdapterProtocol;
  streaming: boolean;
};

beforeEach(() => {
  jest.clearAllMocks();
  jest.mocked(startAudio).mockResolvedValue(true);
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.useRealTimers();
});

async function setup(sound: boolean, props: Partial<Props> = {}) {
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv(), core);
  settings.set('session.sound', sound);
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <SettingsProvider store={settings}>
        <CoreProvider core={core}>{children}</CoreProvider>
      </SettingsProvider>
    );
  }
  const view = await renderHook(
    (p: Props) => useAdapterAudio(p.sessionId, p.protocol, p.streaming),
    {
      wrapper: Wrapper,
      initialProps: {
        sessionId: ID,
        protocol: 'rdp',
        streaming: true,
        ...props,
      } as Props,
    },
  );
  return { ...view, core, settings };
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
  });
}

function logWrites(core: FakeCore) {
  return core.calls.filter(call => call.method === 'logs.write');
}

it('rdp_streaming_starts playback once with sound on', async () => {
  await setup(true);
  expect(startAudio).toHaveBeenCalledTimes(1);
  expect(startAudio).toHaveBeenCalledWith(ID);
  expect(stopAudio).not.toHaveBeenCalled();
});

it('vnc_never_starts playback, even when sound is toggled', async () => {
  const view = await setup(true, { protocol: 'vnc' });
  await act(() => view.settings.set('session.sound', false));
  await act(() => view.settings.set('session.sound', true));
  expect(startAudio).not.toHaveBeenCalled();
  expect(stopAudio).not.toHaveBeenCalled();
  expect(setAudioMuted).not.toHaveBeenCalled();
});

it('stops_on_end once and not again on unmount', async () => {
  const view = await setup(true);
  await view.rerender({ sessionId: ID, protocol: 'rdp', streaming: false });
  expect(stopAudio).toHaveBeenCalledTimes(1);
  await act(() => view.unmount());
  expect(stopAudio).toHaveBeenCalledTimes(1);
  expect(startAudio).toHaveBeenCalledTimes(1);
});

it('waits for streaming and a session id before starting', async () => {
  const view = await setup(true, { sessionId: null, streaming: false });
  await view.rerender({ sessionId: ID, protocol: 'rdp', streaming: false });
  expect(startAudio).not.toHaveBeenCalled();
  await view.rerender({ sessionId: ID, protocol: 'rdp', streaming: true });
  expect(startAudio).toHaveBeenCalledTimes(1);
  await act(() => view.unmount());
  expect(stopAudio).toHaveBeenCalledTimes(1);
});

it('sound_off_no_start until sound is turned on', async () => {
  const view = await setup(false);
  expect(startAudio).not.toHaveBeenCalled();
  await act(() => view.settings.set('session.sound', true));
  expect(startAudio).toHaveBeenCalledTimes(1);
  expect(startAudio).toHaveBeenCalledWith(ID);
});

it('toggle_mutes locally without sessions.setAudio', async () => {
  const view = await setup(true);
  await act(() => view.settings.set('session.sound', false));
  expect(setAudioMuted).toHaveBeenCalledTimes(1);
  expect(setAudioMuted).toHaveBeenCalledWith(true);
  expect(stopAudio).toHaveBeenCalledTimes(1);

  await act(() => view.settings.set('session.sound', true));
  expect(setAudioMuted).toHaveBeenLastCalledWith(false);
  expect(startAudio).toHaveBeenCalledTimes(2);
  expect(
    view.core.calls.some(call => call.method === 'sessions.setAudio'),
  ).toBe(false);
});

it('start_failure_logged_once across two rejected starts', async () => {
  jest
    .mocked(startAudio)
    .mockRejectedValueOnce(new Error('no audio route'))
    .mockRejectedValueOnce(new Error('no audio route'));
  const view = await setup(true);
  await flush();
  await view.rerender({ sessionId: ID, protocol: 'rdp', streaming: false });
  await view.rerender({ sessionId: ID, protocol: 'rdp', streaming: true });
  await flush();
  expect(startAudio).toHaveBeenCalledTimes(2);
  expect(logWrites(view.core)).toEqual([
    {
      method: 'logs.write',
      args: ['warn', 'audio', 'adapter startAudio failed'],
    },
  ]);
});

it('counts a native start that resolves false as a failure', async () => {
  jest.mocked(startAudio).mockResolvedValue(false);
  const view = await setup(true);
  await flush();
  expect(logWrites(view.core)).toEqual([
    {
      method: 'logs.write',
      args: ['warn', 'audio', 'adapter startAudio failed'],
    },
  ]);
});

async function emitPhase(core: FakeCore, phase: AdapterPhase) {
  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: ID,
      protocol: 'rdp',
      phase,
      credError: false,
      ...(phase === 'streaming' ? { width: 1920, height: 1080 } : {}),
    });
  });
}

it('the adapter page plays an RDP session while it streams', async () => {
  jest.useFakeTimers();
  const core = createFakeCore();
  installFakeAdapters(core);
  const route = mockRoute('AdapterSession', {
    target: { protocol: 'rdp', hostId: 'h1', addr: '100.64.0.9' },
    name: 'Windows PC',
    addrKind: 'tailnet',
  });
  const view = await renderWithProviders(
    <AdapterSessionScreen
      navigation={mockNavigation<'AdapterSession'>()}
      route={route}
    />,
    { core, settings: { 'session.sound': true } },
  );
  clients.push(view.queryClient);
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  await emitPhase(core, 'connecting');
  expect(startAudio).not.toHaveBeenCalled();

  await emitPhase(core, 'streaming');
  expect(startAudio).toHaveBeenCalledTimes(1);
  expect(startAudio).toHaveBeenCalledWith(ID);

  await emitPhase(core, 'ended');
  expect(stopAudio).toHaveBeenCalledTimes(1);
});
