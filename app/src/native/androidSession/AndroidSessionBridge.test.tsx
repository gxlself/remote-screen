import type { ReactNode } from 'react';
import { act, render, waitFor } from '@testing-library/react-native';
import { Platform } from 'react-native';
import { CoreProvider } from '../rscore/CoreContext';
import { createFakeCore } from '../rscore/fakeCore';
import {
  SessionsStoreProvider,
  createSessionsStore,
} from '../../features/session/store';
import {
  SettingsProvider,
  createSettingsStore,
} from '../../features/settings/store';
import { useLockState } from '../../features/lock/lockState';
import { MemoryKv } from '../../features/storage/kv';
import type { SessionSummary } from '../rscore/types';
import AndroidSessionBridge from './AndroidSessionBridge';
import type {
  ActiveSession,
  NotificationAction,
  TileConnect,
} from '../specs/NativeRsAndroidSession';

const mockSetActiveSessions = jest.fn<void, [ActiveSession[]]>();
const mockSetTileTarget = jest.fn<void, [string, string]>();
const mockTakePendingTileConnect = jest.fn<string, []>();
const mockRemoveTileListener = jest.fn();
const mockRemoveNotificationListener = jest.fn();
const mockNavigationListeners = new Map<string, () => void>();
let notificationListener: ((event: NotificationAction) => void) | undefined;
let tileListener: ((event: TileConnect) => void) | undefined;

jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    addListener: jest.fn(),
    getCurrentRoute: jest.fn(),
    isReady: jest.fn(),
    navigate: jest.fn(),
  },
}));

const mockNavigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as {
  addListener: jest.Mock;
  getCurrentRoute: jest.Mock;
  isReady: jest.Mock;
  navigate: jest.Mock;
};

jest.mock('./androidSession', () => ({
  setActiveSessions: (sessions: ActiveSession[]) =>
    mockSetActiveSessions(sessions),
  setTileTarget: (hostId: string, label: string) =>
    mockSetTileTarget(hostId, label),
  takePendingTileConnect: () => mockTakePendingTileConnect(),
  subscribeTileConnect: (listener: (event: TileConnect) => void) => {
    tileListener = listener;
    return () => {
      tileListener = undefined;
      mockRemoveTileListener();
    };
  },
  subscribeNotificationAction: (
    listener: (event: NotificationAction) => void,
  ) => {
    notificationListener = listener;
    return () => {
      notificationListener = undefined;
      mockRemoveNotificationListener();
    };
  },
}));

const originalPlatform = Platform.OS;
const first: SessionSummary = {
  id: 'session-1',
  hostId: 'host-1',
  hostName: 'Studio Mac mini',
  phase: 'streaming',
  role: null,
  path: null,
  transport: null,
  startedAt: 1_800_000_000_000_001,
  displayId: null,
};
const second: SessionSummary = {
  ...first,
  id: 'session-2',
  hostId: 'host-2',
  hostName: 'Desk Mac',
  startedAt: 1_800_000_000_000_999,
};

function wrapper(
  core: ReturnType<typeof createFakeCore>,
  store: ReturnType<typeof createSessionsStore>,
  settings = createSettingsStore(
    new MemoryKv({ 'settings.onboarding.done': 'true' }),
  ),
) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>
      <SettingsProvider store={settings}>
        <SessionsStoreProvider store={store}>{children}</SessionsStoreProvider>
      </SettingsProvider>
    </CoreProvider>
  );
}

beforeEach(() => {
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: 'android',
  });
  mockSetActiveSessions.mockClear();
  mockSetTileTarget.mockClear();
  mockTakePendingTileConnect.mockReset().mockReturnValue('');
  mockRemoveTileListener.mockClear();
  mockRemoveNotificationListener.mockClear();
  mockNavigationListeners.clear();
  mockNavigationRef.addListener
    .mockReset()
    .mockImplementation((event, listener) => {
      mockNavigationListeners.set(event, listener);
      return () => mockNavigationListeners.delete(event);
    });
  mockNavigationRef.getCurrentRoute.mockReturnValue({ name: 'MainTabs' });
  mockNavigationRef.isReady.mockReturnValue(true);
  mockNavigationRef.navigate.mockClear();
  notificationListener = undefined;
  tileListener = undefined;
  useLockState.setState({ locked: false });
});

afterEach(() => {
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: originalPlatform,
  });
});

it('does not call Android APIs on iOS', async () => {
  Object.defineProperty(Platform, 'OS', { configurable: true, value: 'ios' });
  const core = createFakeCore();
  const store = createSessionsStore();
  store.seed([first]);
  const rendered = await render(<AndroidSessionBridge />, {
    wrapper: wrapper(core, store),
  });
  expect(mockSetActiveSessions).not.toHaveBeenCalled();
  expect(mockSetTileTarget).not.toHaveBeenCalled();
  expect(notificationListener).toBeUndefined();
  expect(tileListener).toBeUndefined();
  expect(mockTakePendingTileConnect).not.toHaveBeenCalled();
  expect(mockNavigationRef.addListener).not.toHaveBeenCalled();
  await rendered.unmount();
});

it('publishes sorted active sessions, recent tile target, and disconnect actions', async () => {
  const core = createFakeCore();
  const store = createSessionsStore();
  store.seed([second, first]);
  const rendered = await render(<AndroidSessionBridge />, {
    wrapper: wrapper(core, store),
  });
  await waitFor(() => expect(mockSetActiveSessions).toHaveBeenCalledTimes(1));
  expect(mockSetActiveSessions).toHaveBeenLastCalledWith([
    {
      sessionId: first.id,
      title: first.hostName,
      kind: 'desktop',
      startedAtMs: 1_800_000_000_000,
    },
    {
      sessionId: second.id,
      title: second.hostName,
      kind: 'desktop',
      startedAtMs: 1_800_000_000_000,
    },
  ]);
  expect(mockSetTileTarget).toHaveBeenCalledWith(
    second.hostId,
    second.hostName,
  );
  expect(notificationListener).toBeDefined();

  await act(async () => {
    notificationListener?.({ action: 'disconnect', sessionId: first.id });
  });
  expect(core.calls).toContainEqual({
    method: 'sessions.close',
    args: [first.id],
  });
  const closeCalls = core.calls.filter(
    call => call.method === 'sessions.close',
  );
  await act(async () => {
    notificationListener?.({ action: 'open', sessionId: second.id });
  });
  expect(core.calls.filter(call => call.method === 'sessions.close')).toEqual(
    closeCalls,
  );

  await rendered.rerender(<AndroidSessionBridge />);
  expect(mockSetActiveSessions).toHaveBeenCalledTimes(1);
  await act(async () => {
    store.setState(state => ({
      ...state,
      [first.id]: { ...state[first.id], phase: 'ended' },
    }));
  });
  await waitFor(() =>
    expect(mockSetActiveSessions).toHaveBeenLastCalledWith([
      {
        sessionId: second.id,
        title: second.hostName,
        kind: 'desktop',
        startedAtMs: 1_800_000_000_000,
      },
    ]),
  );
  await rendered.unmount();
  expect(mockSetActiveSessions).toHaveBeenLastCalledWith([]);
});

it('connects to the pending tile host when mounted', async () => {
  mockTakePendingTileConnect.mockReturnValue('host-pending');
  const rendered = await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), createSessionsStore()),
  });
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: 'host-pending',
  });
  await rendered.rerender(<AndroidSessionBridge />);
  expect(mockTakePendingTileConnect).toHaveBeenCalledTimes(1);
  expect(mockNavigationRef.navigate).toHaveBeenCalledTimes(1);
});

it.each(['ready', 'state'])(
  'waits for navigation to be ready and rechecks on %s',
  async event => {
    mockNavigationRef.isReady.mockReturnValue(false);
    mockTakePendingTileConnect.mockReturnValue('host-pending');
    await render(<AndroidSessionBridge />, {
      wrapper: wrapper(createFakeCore(), createSessionsStore()),
    });
    expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
    await act(async () => mockNavigationListeners.get(event)?.());
    expect(mockNavigationRef.navigate).not.toHaveBeenCalled();

    mockNavigationRef.isReady.mockReturnValue(true);
    await act(async () => mockNavigationListeners.get(event)?.());
    expect(mockNavigationRef.navigate).toHaveBeenCalledTimes(1);
    expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
      hostId: 'host-pending',
    });
    await act(async () => mockNavigationListeners.get(event)?.());
    expect(mockNavigationRef.navigate).toHaveBeenCalledTimes(1);
  },
);

it('keeps a tile request pending while locked and navigates after unlock', async () => {
  useLockState.setState({ locked: true });
  mockTakePendingTileConnect.mockReturnValue('host-pending');
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), createSessionsStore()),
  });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
  await act(async () => mockNavigationListeners.get('ready')?.());
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();

  await act(async () => useLockState.getState().setLocked(false));
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: 'host-pending',
  });
});

it('discards tile and notification requests before onboarding is done', async () => {
  const settings = createSettingsStore(new MemoryKv());
  const store = createSessionsStore();
  store.seed([first]);
  mockTakePendingTileConnect.mockReturnValue('host-pending');
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), store, settings),
  });
  await act(async () => {
    tileListener?.({ hostId: 'host-runtime' });
    notificationListener?.({ action: 'open', sessionId: first.id });
  });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();

  await act(async () => settings.set('onboarding.done', true));
  await act(async () => mockNavigationListeners.get('state')?.());
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});

it('does not navigate when already on the same session', async () => {
  const store = createSessionsStore();
  store.seed([first]);
  mockNavigationRef.getCurrentRoute.mockReturnValue({
    name: 'Session',
    params: { hostId: first.hostId, sessionId: first.id },
  });
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), store),
  });
  await act(async () => {
    tileListener?.({ hostId: '' });
    tileListener?.({ hostId: first.hostId });
    notificationListener?.({ action: 'open', sessionId: first.id });
  });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});

it('navigates to the live session when a notification open action arrives', async () => {
  const store = createSessionsStore();
  store.seed([first, second]);
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), store),
  });
  await act(async () => {
    notificationListener?.({ action: 'open', sessionId: second.id });
  });
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: second.hostId,
    sessionId: second.id,
  });
});

it('resumes the most recent session for an empty runtime tile request', async () => {
  const store = createSessionsStore();
  store.seed([second, first]);
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), store),
  });
  await act(async () => tileListener?.({ hostId: '' }));
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: second.hostId,
    sessionId: second.id,
  });
});

it('resolves runtime requests against the latest session state', async () => {
  const store = createSessionsStore();
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), store),
  });
  await act(async () => store.seed([first]));
  await act(async () => tileListener?.({ hostId: first.hostId }));
  expect(mockNavigationRef.navigate).toHaveBeenLastCalledWith('Session', {
    hostId: first.hostId,
    sessionId: first.id,
  });
  mockNavigationRef.navigate.mockClear();

  await act(async () => {
    store.setState(state => ({
      ...state,
      [first.id]: { ...state[first.id], phase: 'ended' },
    }));
  });
  await act(async () => {
    notificationListener?.({ action: 'open', sessionId: first.id });
    tileListener?.({ hostId: '' });
  });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
  await act(async () => tileListener?.({ hostId: first.hostId }));
  expect(mockNavigationRef.navigate).toHaveBeenLastCalledWith('Session', {
    hostId: first.hostId,
  });
});

it('keeps the latest request while waiting and checks both lock and readiness', async () => {
  useLockState.setState({ locked: true });
  mockNavigationRef.isReady.mockReturnValue(false);
  const store = createSessionsStore();
  store.seed([first]);
  await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), store),
  });
  await act(async () => tileListener?.({ hostId: 'host-new' }));
  await act(async () => {
    notificationListener?.({ action: 'open', sessionId: first.id });
  });
  await act(async () => useLockState.getState().setLocked(false));
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
  mockNavigationRef.isReady.mockReturnValue(true);
  await act(async () => mockNavigationListeners.get('ready')?.());
  expect(mockNavigationRef.navigate).toHaveBeenCalledTimes(1);
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('Session', {
    hostId: first.hostId,
    sessionId: first.id,
  });
});

it('removes native and navigation listeners and discards pending navigation on unmount', async () => {
  mockNavigationRef.isReady.mockReturnValue(false);
  mockTakePendingTileConnect.mockReturnValue('host-pending');
  const rendered = await render(<AndroidSessionBridge />, {
    wrapper: wrapper(createFakeCore(), createSessionsStore()),
  });
  await rendered.unmount();
  expect(mockRemoveTileListener).toHaveBeenCalledTimes(1);
  expect(mockRemoveNotificationListener).toHaveBeenCalledTimes(1);
  expect(mockNavigationListeners.size).toBe(0);
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});
