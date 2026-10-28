import { act, cleanup } from '@testing-library/react-native';
import { AppState, Platform } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import type { PipChange } from '../../native/specs/NativeRsAndroidSession';
import SessionLifecycle from './SessionLifecycle';

const mockIsInPip = jest.fn(() => false);
let mockPipListener: ((event: PipChange) => void) | undefined;
const mockRemovePipListener = jest.fn();
const mockIsPipActive = jest.fn(() => false);
let mockIosPipListener:
  | ((event: { sessionId: string; active: boolean; reason: string }) => void)
  | undefined;
const mockRemoveIosPipListener = jest.fn();

jest.mock('../../native/androidSession/androidSession', () => ({
  isInPip: () => mockIsInPip(),
  subscribePipChange: (listener: (event: PipChange) => void) => {
    mockPipListener = listener;
    return mockRemovePipListener;
  },
}));

jest.mock('../pip/nativePip', () => ({
  isPipActive: () => mockIsPipActive(),
  subscribePipChange: (
    listener: (event: {
      sessionId: string;
      active: boolean;
      reason: string;
    }) => void,
  ) => {
    mockIosPipListener = listener;
    return mockRemoveIosPipListener;
  },
}));

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
  mockPipListener = undefined;
});

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'android');
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'active',
  });
  mockIsInPip.mockReturnValue(false);
  mockIsPipActive.mockReturnValue(false);
  mockRemovePipListener.mockClear();
  mockRemoveIosPipListener.mockClear();
  mockIosPipListener = undefined;
});

it('pauses and resumes all live sessions, including collapsed sessions, and skips ended ones', async () => {
  let change: (state: AppStateStatus) => void = () => {};
  const remove = jest.fn();
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_, handler) => {
    change = handler;
    return { remove };
  });
  const core = createFakeCore({
    sessions: [
      liveSession,
      { ...liveSession, id: 's-second', phase: 'reconnecting' },
      { ...liveSession, id: 's-ended', phase: 'ended' },
    ],
  });
  const view = await renderWithProviders(<SessionLifecycle />, { core });
  await act(() => change('inactive'));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await act(() => change('background'));
  await act(() => change('active'));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([
    { method: 'sessions.background', args: [liveSession.id, true] },
    { method: 'sessions.background', args: ['s-second', true] },
    { method: 'sessions.background', args: [liveSession.id, false] },
    { method: 'sessions.background', args: ['s-second', false] },
  ]);

  // The subscription must read updated store contents without remounting.
  await act(() =>
    core.emit({
      type: 'session.error',
      sessionId: 's-second',
      hostId: liveSession.hostId,
      reason: 'background',
    }),
  );
  core.calls.length = 0;
  await act(() =>
    core.emit({
      type: 'session.state',
      sessionId: 's-new',
      hostId: liveSession.hostId,
      phase: 'connecting',
    }),
  );
  await act(() => change('active'));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([
    { method: 'sessions.background', args: [liveSession.id, false] },
    { method: 'sessions.background', args: ['s-new', false] },
  ]);
  await view.unmount();
  expect(remove).toHaveBeenCalledTimes(1);
  expect(mockIsPipActive).not.toHaveBeenCalled();
  expect(mockIosPipListener).toBeUndefined();
  expect(core.calls.some(call => call.method === 'sessions.close')).toBe(false);
  view.queryClient.clear();
});

it('does not background sessions when iOS enters the background in PiP', async () => {
  let change: (state: AppStateStatus) => void = () => {};
  jest.replaceProperty(Platform, 'OS', 'ios');
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_, handler) => {
    change = handler;
    return { remove: jest.fn() };
  });
  mockIsPipActive.mockReturnValue(true);
  const core = createFakeCore({ sessions: [liveSession] });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() => change('background'));

  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  expect(mockIosPipListener).toBeDefined();
  await view.unmount();
  view.queryClient.clear();
});

it('backgrounds iOS sessions 500ms after PiP closes in the background', async () => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'background',
  });
  jest.spyOn(AppState, 'addEventListener').mockImplementation(() => ({
    remove: jest.fn(),
  }));
  const core = createFakeCore({
    sessions: [liveSession, { ...liveSession, id: 's-second' }],
  });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() =>
    mockIosPipListener?.({
      sessionId: liveSession.id,
      active: false,
      reason: 'closed',
    }),
  );
  await act(() => jest.advanceTimersByTime(499));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await act(() => jest.advanceTimersByTime(1));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([
    { method: 'sessions.background', args: [liveSession.id, true] },
    { method: 'sessions.background', args: ['s-second', true] },
  ]);
  await view.unmount();
  view.queryClient.clear();
});

it('does not background iOS sessions if the app returns before the delay', async () => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'background',
  });
  jest.spyOn(AppState, 'addEventListener').mockImplementation(() => ({
    remove: jest.fn(),
  }));
  const core = createFakeCore({ sessions: [liveSession] });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() =>
    mockIosPipListener?.({
      sessionId: liveSession.id,
      active: false,
      reason: 'closed',
    }),
  );
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'active',
  });
  await act(() => jest.advanceTimersByTime(500));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await view.unmount();
  view.queryClient.clear();
});

it('keeps the regular iOS background behavior outside PiP', async () => {
  let change: (state: AppStateStatus) => void = () => {};
  jest.replaceProperty(Platform, 'OS', 'ios');
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_, handler) => {
    change = handler;
    return { remove: jest.fn() };
  });
  mockIsPipActive.mockReturnValue(false);
  const core = createFakeCore({ sessions: [liveSession] });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() => change('background'));

  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([{ method: 'sessions.background', args: [liveSession.id, true] }]);
  await view.unmount();
  view.queryClient.clear();
});

it('does not background sessions when Android enters the background in PiP', async () => {
  let change: (state: AppStateStatus) => void = () => {};
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_, handler) => {
    change = handler;
    return { remove: jest.fn() };
  });
  mockIsInPip.mockReturnValue(true);
  const core = createFakeCore({ sessions: [liveSession] });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() => change('background'));

  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await view.unmount();
  view.queryClient.clear();
});

it('backgrounds live sessions 500ms after PiP closes while the app stays backgrounded', async () => {
  jest.useFakeTimers();
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'background',
  });
  jest.spyOn(AppState, 'addEventListener').mockImplementation(() => ({
    remove: jest.fn(),
  }));
  const core = createFakeCore({
    sessions: [liveSession, { ...liveSession, id: 's-second' }],
  });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() => mockPipListener?.({ inPip: false }));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await act(() => jest.advanceTimersByTime(499));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await act(() => jest.advanceTimersByTime(1));
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([
    { method: 'sessions.background', args: [liveSession.id, true] },
    { method: 'sessions.background', args: ['s-second', true] },
  ]);

  await view.unmount();
  view.queryClient.clear();
});

it('does not background sessions if the app becomes active before the PiP delay', async () => {
  jest.useFakeTimers();
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'background',
  });
  jest.spyOn(AppState, 'addEventListener').mockImplementation(() => ({
    remove: jest.fn(),
  }));
  const core = createFakeCore({ sessions: [liveSession] });
  const view = await renderWithProviders(<SessionLifecycle />, { core });

  await act(() => mockPipListener?.({ inPip: false }));
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'active',
  });
  await act(() => jest.advanceTimersByTime(500));

  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([]);
  await view.unmount();
  view.queryClient.clear();
});
