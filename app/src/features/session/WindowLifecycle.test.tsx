import { act, cleanup } from '@testing-library/react-native';
import { Platform } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import type { Spec, WindowStateEvent } from '../../native/specs/NativeRsWindow';
import { renderWithProviders } from '../../test/render';
import type { SwitcherItem } from './switcher';
import { WindowLifecycle } from './WindowLifecycle';
import {
  MAIN_WINDOW,
  registerWindowPage,
  releaseWindowPage,
  resetWindows,
  windows,
} from './windows';

let mockListener: ((event: WindowStateEvent) => void) | undefined;
const mockRefresh = jest.fn();
let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsWindow', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

const mac: SwitcherItem = {
  kind: 'host',
  id: liveSession.id,
  hostId: liveSession.hostId,
  name: 'Studio Mac mini',
  status: 'connected',
  startedAtUs: 100,
};
const second: SwitcherItem = { ...mac, id: 's-second', name: 'Mac 2' };

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  mockListener = undefined;
  mockRefresh.mockReset();
  mockNativeModule = {
    getConstants: () => ({ supported: true }),
    open: jest.fn(),
    activate: jest.fn(),
    close: jest.fn(),
    refresh: mockRefresh,
    onWindowState: jest.fn(listener => {
      mockListener = listener;
      return { remove: jest.fn() };
    }),
  };
  resetWindows();
});

afterEach(async () => {
  await cleanup();
  mockNativeModule = null;
  jest.restoreAllMocks();
});

async function mount() {
  const core = createFakeCore({
    sessions: [liveSession, { ...liveSession, id: 's-second' }],
  });
  await renderWithProviders(<WindowLifecycle />, { core });
  const send = (windowId: string, state: string) =>
    act(() => mockListener?.({ windowId, state }));
  const background = () =>
    core.calls
      .filter(call => call.method === 'sessions.background')
      .map(call => call.args);
  return { core, send, background };
}

it('follows the native windows and asks for their state once', async () => {
  const { send } = await mount();
  expect(mockRefresh).toHaveBeenCalledTimes(1);
  await send(MAIN_WINDOW, 'active');
  await send('w1', 'inactive');
  expect(windows.getState().phases).toEqual({
    [MAIN_WINDOW]: 'active',
    w1: 'inactive',
  });
});

it('pauses the session of a window off screen and resumes it when back', async () => {
  const { send, background } = await mount();
  await act(() => {
    registerWindowPage('p1', MAIN_WINDOW, mac);
    registerWindowPage('p2', 'w1', second);
  });
  await send(MAIN_WINDOW, 'active');
  await send('w1', 'active');
  // Split view: both windows on screen, both sessions play.
  expect(background()).toEqual([]);

  await send('w1', 'background');
  expect(background()).toEqual([['s-second', true]]);
  await send('w1', 'inactive');
  expect(background()).toEqual([
    ['s-second', true],
    ['s-second', false],
  ]);
});

it('leaves the whole app going away to the app-wide lifecycle', async () => {
  const { send, background } = await mount();
  await act(() => registerWindowPage('p1', 'w1', second));
  await send(MAIN_WINDOW, 'active');
  await send('w1', 'background');
  expect(background()).toEqual([['s-second', true]]);
  await send(MAIN_WINDOW, 'background');
  // Nothing on screen: SessionLifecycle pauses everything; nothing here.
  expect(background()).toEqual([['s-second', true]]);
  // Back with only the main window: its window is still off screen.
  await send(MAIN_WINDOW, 'active');
  expect(background()).toEqual([
    ['s-second', true],
    ['s-second', true],
  ]);
});

it('resumes a Host session whose window closed', async () => {
  const { send, background } = await mount();
  await act(() => registerWindowPage('p1', 'w1', second));
  await send(MAIN_WINDOW, 'active');
  await send('w1', 'background');
  await act(() => releaseWindowPage('p1'));
  await send('w1', 'closed');
  expect(background()).toEqual([
    ['s-second', true],
    ['s-second', false],
  ]);
});
