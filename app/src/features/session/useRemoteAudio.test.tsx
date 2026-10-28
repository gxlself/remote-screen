import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { ReactNode } from 'react';
import { setAudioMuted, startAudio, stopAudio } from '../../native/audio/audio';
import { CoreProvider } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { Computer } from '../../native/rscore/types';
import { MemoryKv } from '../storage/kv';
import { createSettingsStore, SettingsProvider } from '../settings/store';
import { AUDIO_OPUS_BIT } from './remoteAudio';
import { useRemoteAudio } from './useRemoteAudio';

jest.mock('../../native/audio/audio', () => ({
  startAudio: jest.fn().mockResolvedValue(true),
  stopAudio: jest.fn(),
  setAudioMuted: jest.fn(),
}));

const audioHost = { features: AUDIO_OPUS_BIT } as Computer;
const silentHost = { features: 0 } as Computer;

beforeEach(() => {
  jest.clearAllMocks();
  jest.mocked(startAudio).mockResolvedValue(true);
});

afterEach(async () => {
  await cleanup();
});

async function setup(sound: boolean, host: Computer | undefined = audioHost) {
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
    (props: { sessionId: string | null; host: Computer | undefined }) =>
      useRemoteAudio(props.sessionId, props.host),
    {
      wrapper: Wrapper,
      initialProps: { sessionId: 's1' as string | null, host },
    },
  );
  return { ...view, core, settings };
}

it('starts playback once when the session opens with sound enabled', async () => {
  const view = await setup(true);
  expect(startAudio).toHaveBeenCalledTimes(1);
  expect(startAudio).toHaveBeenCalledWith('s1');
  expect(
    view.core.calls.some(call => call.method === 'sessions.setAudio'),
  ).toBe(false);
});

it('stops immediately and tells the host when sound is turned off', async () => {
  const view = await setup(true);
  await act(() => view.settings.set('session.sound', false));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.setAudio',
    args: ['s1', false],
  });
  expect(setAudioMuted).toHaveBeenCalledWith(true);
  expect(stopAudio).toHaveBeenCalledTimes(1);
});

it('resumes playback and unmutes when sound is turned back on', async () => {
  const view = await setup(true);
  await act(() => view.settings.set('session.sound', false));
  await act(() => view.settings.set('session.sound', true));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.setAudio',
    args: ['s1', true],
  });
  expect(setAudioMuted).toHaveBeenLastCalledWith(false);
  expect(startAudio).toHaveBeenCalledTimes(2);
  expect(startAudio).toHaveBeenLastCalledWith('s1');
});

it('stops playback on unmount', async () => {
  const view = await setup(true);
  await act(() => view.unmount());
  expect(stopAudio).toHaveBeenCalledTimes(1);
});

it('swallows unsupported setAudio errors and warns once', async () => {
  const view = await setup(true);
  const unsupported = () => new RsCoreError('unsupported', 'sessions.setAudio');
  view.core.failNext('sessions.setAudio', unsupported());
  view.core.failNext('sessions.setAudio', unsupported());

  await act(() => view.settings.set('session.sound', false));
  await act(() => view.settings.set('session.sound', true));

  expect(view.core.calls.filter(call => call.method === 'logs.write')).toEqual([
    {
      method: 'logs.write',
      args: ['warn', 'audio', 'sessions.setAudio unsupported'],
    },
  ]);
});

it('does not start playback with a host older than 0.4.0', async () => {
  await setup(true, silentHost);
  expect(startAudio).not.toHaveBeenCalled();
});

it('does not surface a rejected native start', async () => {
  jest.mocked(startAudio).mockRejectedValueOnce(new Error('no audio route'));
  const view = await setup(true);
  await act(async () => {
    await Promise.resolve();
  });
  expect(view.core.calls.filter(call => call.method === 'logs.write')).toEqual([
    { method: 'logs.write', args: ['warn', 'audio', 'startAudio failed'] },
  ]);
});
