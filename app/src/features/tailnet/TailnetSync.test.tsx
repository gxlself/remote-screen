import type { ReactNode } from 'react';
import { act, cleanup, renderHook } from '@testing-library/react-native';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  tailnetNeedsLogin,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { useEmbeddedSwitch } from './useEmbeddedSwitch';
import TailnetSync from './TailnetSync';
import {
  LOGIN_WINDOW_MS,
  resetTailnetSwitchStore,
  tailnetSwitchStore,
} from './switchStore';

const originalCurrentState = AppState.currentState;
/** The sync only runs while 外网访问 (Tailscale) is on. */
const ON = { 'tailnet.enabled': true } as const;

function spyOnAppState() {
  const listeners: ((state: AppStateStatus) => void)[] = [];
  const remove = jest.fn();
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_type, cb) => {
    listeners.push(cb as (state: AppStateStatus) => void);
    return { remove };
  });
  return { listeners, remove };
}

async function renderSwitchAndSync() {
  const core = createFakeCore({ tailnetAvailable: true });
  const providers = await renderWithProviders(<></>, {
    core,
    settings: { ...ON, 'tailnet.controlUrl': 'https://hs.example.com' },
  });
  await providers.unmount();
  const view = await renderHook(() => useEmbeddedSwitch(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <providers.wrapper>
        <TailnetSync />
        {children}
      </providers.wrapper>
    ),
  });
  return { ...view, core, platform: providers.platform };
}

beforeEach(() => {
  resetTailnetSwitchStore();
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  Object.defineProperty(AppState, 'currentState', { value: 'active' });
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  Object.defineProperty(AppState, 'currentState', {
    value: originalCurrentState,
  });
  jest.useRealTimers();
});

it('pushes app active changes and deduplicates values', async () => {
  const { listeners } = spyOnAppState();
  const core = createFakeCore();
  await renderWithProviders(<TailnetSync />, { core, settings: ON });
  const values = () =>
    core.calls
      .filter(
        call =>
          call.method === 'settings.set' && call.args[0] === 'device.appActive',
      )
      .map(call => call.args);
  expect(values()).toEqual([['device.appActive', true]]);
  await act(() => listeners[0]?.('background'));
  expect(values().at(-1)).toEqual(['device.appActive', false]);
  await act(() => listeners[0]?.('inactive'));
  expect(values()).toHaveLength(2);
  await act(() => listeners[0]?.('active'));
  await act(() => listeners[0]?.('active'));
  expect(values()).toEqual([
    ['device.appActive', true],
    ['device.appActive', false],
    ['device.appActive', true],
  ]);
});

it('pushes nothing and opens no login while Tailscale is switched off', async () => {
  const { listeners } = spyOnAppState();
  const core = createFakeCore({ tailnetAvailable: true });
  const view = await renderWithProviders(<TailnetSync />, { core });
  tailnetSwitchStore.setState({
    loginWindowUntilMs: Date.now() + LOGIN_WINDOW_MS,
  });
  await act(() => core.setTailnet({ status: tailnetNeedsLogin }));
  expect(listeners).toEqual([]);
  expect(
    core.calls.filter(
      call =>
        call.method === 'settings.set' && call.args[0] === 'device.appActive',
    ),
  ).toEqual([]);
  expect(view.platform.opened).toEqual([]);
});

it('seeds a background state and removes the subscription on unmount', async () => {
  Object.defineProperty(AppState, 'currentState', { value: 'background' });
  const { listeners, remove } = spyOnAppState();
  const core = createFakeCore();
  const view = await renderWithProviders(<TailnetSync />, { core, settings: ON });
  expect(core.calls).toContainEqual({
    method: 'settings.set',
    args: ['device.appActive', false],
  });
  await view.unmount();
  expect(remove).toHaveBeenCalledTimes(1);
  const before = core.calls.length;
  await act(() => listeners[0]?.('active'));
  expect(core.calls).toHaveLength(before);
});

it('warns once and stops pushing after app active sync is unsupported', async () => {
  const { listeners } = spyOnAppState();
  const core = createFakeCore();
  const warn = jest.spyOn(console, 'warn').mockImplementation(() => {});
  const originalSet = core.settings.set;
  const set = jest
    .spyOn(core.settings, 'set')
    .mockImplementation((key, value) => {
      if (key === 'device.appActive') {
        throw new Error('unsupported');
      }
      originalSet(key, value);
    });
  await renderWithProviders(<TailnetSync />, { core, settings: ON });
  expect(set.mock.calls.filter(([key]) => key === 'device.appActive')).toEqual([
    ['device.appActive', true],
  ]);
  const before = set.mock.calls.length;
  await act(() => listeners[0]?.('background'));
  await act(() => listeners[0]?.('active'));
  expect(warn).toHaveBeenCalledTimes(1);
  expect(warn).toHaveBeenCalledWith('device.appActive sync is unsupported');
  expect(set).toHaveBeenCalledTimes(before);
});

it('opens a browser once after a successful switch', async () => {
  const view = await renderSwitchAndSync();
  expect(await act(() => view.result.current.switchToEmbedded())).toBe(
    'switched',
  );
  await act(() => view.core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.platform.opened).toEqual([
    'https://hs.example.com/register/nodekey-test',
  ]);
  await act(() => view.core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.platform.opened).toHaveLength(1);
  expect(tailnetSwitchStore.getState().loginWindowUntilMs).toBeNull();
});

it('ignores needsLogin events without a switch window', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const view = await renderWithProviders(<TailnetSync />, { core, settings: ON });
  await act(() => core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.platform.opened).toEqual([]);
});

it('ignores the first login event if the switch was over 20 seconds ago', async () => {
  const view = await renderSwitchAndSync();
  await act(() => view.result.current.switchToEmbedded());
  await act(() => jest.advanceTimersByTimeAsync(21_000));
  await act(() => view.core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.platform.opened).toEqual([]);
});

it('includes the exact login deadline and leaves undo available when running', async () => {
  const view = await renderSwitchAndSync();
  await act(() => view.result.current.switchToEmbedded());
  await act(() => jest.advanceTimersByTimeAsync(LOGIN_WINDOW_MS));
  await act(() => view.core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.platform.opened).toHaveLength(1);
  await act(() => view.core.setTailnet({ status: tailnetRunning }));
  expect(tailnetSwitchStore.getState()).toEqual({
    previousMode: 'system',
    openedAuthUrl: null,
    loginWindowUntilMs: null,
  });
  expect(view.result.current.canUndo).toBe(true);
});

it('opens a later login event requested explicitly by openLogin', async () => {
  const view = await renderSwitchAndSync();
  await act(() => view.result.current.openLogin());
  expect(view.core.calls.some(call => call.method === 'tailnet.login')).toBe(
    true,
  );
  await act(() => view.core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.platform.opened).toHaveLength(1);
});
