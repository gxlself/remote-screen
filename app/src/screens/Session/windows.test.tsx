import { act, cleanup } from '@testing-library/react-native';
import { Platform } from 'react-native';
import { WindowProvider } from '../../features/session/WindowContext';
import {
  MAIN_WINDOW,
  registerWindowPage,
  resetWindows,
  windows,
} from '../../features/session/windows';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import type { Spec } from '../../native/specs/NativeRsWindow';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionScreen from './index';

const mockActivate = jest.fn<void, [string]>();
let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsWindow', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

const session = { ...liveSession, id: 's1' };

beforeEach(() => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  mockActivate.mockReset();
  mockNativeModule = {
    getConstants: () => ({ supported: true }),
    open: jest.fn(),
    activate: mockActivate,
    close: jest.fn(),
    refresh: jest.fn(),
    onWindowState: jest.fn(() => ({ remove: jest.fn() })),
  };
  resetWindows();
});

afterEach(async () => {
  await cleanup();
  mockNativeModule = null;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

async function mount(windowId: string) {
  const core = createFakeCore({ sessions: [session] });
  const navigation = mockNavigation<'Session'>();
  const route = mockRoute('Session', {
    hostId: session.hostId,
    sessionId: session.id,
  });
  await renderWithProviders(
    <WindowProvider windowId={windowId}>
      <SessionScreen navigation={navigation} route={route} />
    </WindowProvider>,
    { core, localDirectory: { peers: [] } },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { core, navigation };
}

it('tells the other windows which session its page shows', async () => {
  await mount(MAIN_WINDOW);
  expect(Object.values(windows.getState().pages)).toEqual([
    expect.objectContaining({
      windowId: MAIN_WINDOW,
      item: expect.objectContaining({ kind: 'host', id: 's1' }),
    }),
  ]);
  await cleanup();
  expect(windows.getState().pages).toEqual({});
});

it('never shows a session that another window shows: that window comes forward', async () => {
  registerWindowPage('elsewhere', 'w1', {
    kind: 'host',
    id: 's1',
    hostId: session.hostId,
    name: 'Studio Mac mini',
    status: 'connected',
    startedAtUs: 1,
  });
  const { core, navigation } = await mount('w2');
  expect(mockActivate).toHaveBeenCalledWith('w1');
  expect(navigation.goBack).toHaveBeenCalled();
  expect(
    Object.values(windows.getState().pages).map(page => page.windowId),
  ).toEqual(['w1']);
  expect(core.calls.some(call => call.method === 'sessions.open')).toBe(false);
});
