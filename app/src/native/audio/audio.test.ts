import { createElement } from 'react';
import { act, fireEvent, render } from '@testing-library/react-native';
import { setAudioMuted, startAudio, stopAudio } from './audio';
import AudioHarness from '../devharness/AudioHarness';
import type { AudioStateEvent, AudioStats, Spec } from '../specs/NativeRsAudio';

const mockStart = jest.fn<Promise<boolean>, [string]>();
const mockStop = jest.fn<void, []>();
const mockSetMuted = jest.fn<void, [boolean]>();
const mockGetStats = jest.fn<Promise<AudioStats>, []>();
const mockRemoveListener = jest.fn();
const mockOnAudioState = jest.fn<ReturnType<Spec['onAudioState']>, [((event: AudioStateEvent) => void)]>();
let mockStateListener: ((event: AudioStateEvent) => void) | undefined;

const stats: AudioStats = {
  running: true,
  packets: 42,
  lost: 1,
  concealed: 1,
  underruns: 0,
  targetMs: 40,
  levelMs: 30,
  outputLatencyMs: 10,
  ioBufferMs: 5,
  route: 'Speaker',
};

let mockNativeModule: Spec | null = null;

jest.mock('../specs/NativeRsAudio', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  mockStart.mockReset();
  mockStop.mockReset();
  mockSetMuted.mockReset();
  mockGetStats.mockReset().mockResolvedValue(stats);
  mockRemoveListener.mockReset();
  mockStateListener = undefined;
  mockOnAudioState.mockReset().mockImplementation(listener => {
    mockStateListener = listener;
    return { remove: mockRemoveListener };
  });
  mockNativeModule = {
    start: mockStart,
    stop: mockStop,
    setMuted: mockSetMuted,
    getStats: mockGetStats,
    onAudioState: mockOnAudioState,
  };
});

afterEach(() => {
  jest.useRealTimers();
});

it('keeps the empty implementation behavior when the native module is absent', async () => {
  mockNativeModule = null;
  await expect(startAudio('42')).resolves.toBe(false);
  expect(() => stopAudio()).not.toThrow();
  expect(() => setAudioMuted(true)).not.toThrow();
});

it('returns false when the native start call throws', async () => {
  mockStart.mockRejectedValueOnce(new Error('audio unavailable'));
  await expect(startAudio('42')).resolves.toBe(false);
  mockStart.mockImplementationOnce(() => { throw new Error('bridge unavailable'); });
  await expect(startAudio('42')).resolves.toBe(false);
});

it('forwards start, stop, and mute calls to the native module', async () => {
  mockStart.mockResolvedValueOnce(true);
  await expect(startAudio('18446744073709551615')).resolves.toBe(true);
  setAudioMuted(true);
  stopAudio();
  expect(mockStart).toHaveBeenCalledWith('18446744073709551615');
  expect(mockSetMuted).toHaveBeenCalledWith(true);
  expect(mockStop).toHaveBeenCalledWith();
});

it('preserves a native start refusal', async () => {
  mockStart.mockResolvedValueOnce(false);
  await expect(startAudio('42')).resolves.toBe(false);
});

it('polls harness stats every 500 ms and removes its event listener on unmount', async () => {
  jest.useFakeTimers();
  const log = jest.fn();
  const view = await render(createElement(AudioHarness, { arg: '42', log }));
  expect(mockGetStats).toHaveBeenCalledTimes(1);
  expect(view.getByText(/"targetMs": 40/)).toBeOnTheScreen();

  await act(async () => { jest.advanceTimersByTime(499); });
  expect(mockGetStats).toHaveBeenCalledTimes(1);
  await act(async () => { jest.advanceTimersByTime(1); });
  expect(mockGetStats).toHaveBeenCalledTimes(2);
  await act(async () => {
    mockStateListener?.({ state: 'interrupted', reason: 'interruption' });
  });
  expect(view.getByText('state: interrupted (interruption)')).toBeOnTheScreen();
  expect(log).toHaveBeenCalledWith('AUDIO-STATE', {
    state: 'interrupted', reason: 'interruption',
  });

  await view.unmount();
  expect(mockRemoveListener).toHaveBeenCalledTimes(1);
  await act(async () => { jest.advanceTimersByTime(500); });
  expect(mockGetStats).toHaveBeenCalledTimes(2);
});

it('runs and logs each harness command', async () => {
  mockStart.mockResolvedValueOnce(true);
  const log = jest.fn();
  const view = await render(createElement(AudioHarness, { arg: '42', log }));
  for (const title of ['开始', '停止', '静音', '取消静音']) {
    await fireEvent.press(view.getByText(title));
  }
  expect(mockStart).toHaveBeenCalledWith('42');
  expect(mockStop).toHaveBeenCalledTimes(1);
  expect(mockSetMuted.mock.calls).toEqual([[true], [false]]);
  for (const action of ['START', 'STOP', 'MUTE', 'UNMUTE']) {
    expect(log).toHaveBeenCalledWith(`AUDIO-${action}`, { sessionId: '42', ok: true });
  }
  await view.unmount();
});
