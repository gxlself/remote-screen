import { act, cleanup, renderHook } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW, tailnetNeedsLogin } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import {
  LOGIN_WINDOW_MS,
  resetTailnetSwitchStore,
  tailnetSwitchStore,
} from './switchStore';
import { useEmbeddedSwitch } from './useEmbeddedSwitch';

type ProviderOptions = Parameters<typeof renderWithProviders>[1];

async function renderSwitch(opts?: ProviderOptions) {
  const providers = await renderWithProviders(<></>, opts);
  await providers.unmount();
  const view = await renderHook(() => useEmbeddedSwitch(), {
    wrapper: providers.wrapper,
  });
  return {
    ...view,
    core: providers.core,
    platform: providers.platform,
    settingsStore: providers.settingsStore,
  };
}

beforeEach(() => {
  resetTailnetSwitchStore();
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
});
afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('returns unavailable without changing any settings', async () => {
  const view = await renderSwitch();
  expect(view.result.current.available).toBe(false);
  const writes = view.core.calls.filter(c => c.method === 'settings.set');
  expect(await act(() => view.result.current.switchToEmbedded())).toBe(
    'unavailable',
  );
  expect(view.settingsStore.get('tailnet.mode')).toBe('system');
  expect(view.core.calls.filter(c => c.method === 'settings.set')).toEqual(
    writes,
  );
  expect(tailnetSwitchStore.getState()).toEqual({
    previousMode: null,
    loginWindowUntilMs: null,
    openedAuthUrl: null,
  });
});

it('treats a failed availability check as unavailable', async () => {
  const view = await renderSwitch({
    core: createFakeCore({ tailnetAvailable: true }),
    settings: { 'tailnet.controlUrl': 'https://hs.example.com' },
  });
  view.core.failNext('tailnet.available', new Error('unsupported'));
  expect(await act(() => view.result.current.switchToEmbedded())).toBe(
    'unavailable',
  );
  expect(view.result.current.available).toBe(false);
  expect(view.settingsStore.get('tailnet.mode')).toBe('system');
});

it('requires a control URL before switching', async () => {
  const view = await renderSwitch({
    core: createFakeCore({ tailnetAvailable: true }),
  });
  expect(view.result.current.available).toBe(true);
  expect(await act(() => view.result.current.switchToEmbedded())).toBe(
    'needsControlUrl',
  );
  expect(view.settingsStore.get('tailnet.mode')).toBe('system');
  expect(view.result.current.canUndo).toBe(false);
});

it.each(['system', 'auto'] as const)(
  'switches and undoes to %s',
  async mode => {
    const view = await renderSwitch({
      core: createFakeCore({ tailnetAvailable: true }),
      settings: {
        'tailnet.mode': mode,
        'tailnet.controlUrl': 'https://hs.example.com',
      },
    });
    expect(await act(() => view.result.current.switchToEmbedded())).toBe(
      'switched',
    );
    expect(view.settingsStore.get('tailnet.mode')).toBe('embedded');
    expect(view.result.current.canUndo).toBe(true);
    expect(tailnetSwitchStore.getState().loginWindowUntilMs).toBe(
      Date.now() + LOGIN_WINDOW_MS,
    );
    await act(() => view.result.current.undo());
    expect(view.settingsStore.get('tailnet.mode')).toBe(mode);
    expect(view.result.current.canUndo).toBe(false);
    expect(tailnetSwitchStore.getState().loginWindowUntilMs).toBeNull();
  },
);

it('preserves the original mode when the switch is repeated', async () => {
  const view = await renderSwitch({
    core: createFakeCore({ tailnetAvailable: true }),
    settings: { 'tailnet.controlUrl': 'https://hs.example.com' },
  });
  await act(() => view.result.current.switchToEmbedded());
  const state = tailnetSwitchStore.getState();
  await act(() => jest.advanceTimersByTimeAsync(1_000));
  await act(() => view.result.current.switchToEmbedded());
  expect(tailnetSwitchStore.getState()).toEqual(state);
  await act(() => view.result.current.undo());
  const writes = view.core.calls.filter(c => c.method === 'settings.set');
  await act(() => view.result.current.undo());
  expect(view.core.calls.filter(c => c.method === 'settings.set')).toEqual(
    writes,
  );
});

it('normalizes a saved URL and does not change settings for invalid input', async () => {
  const view = await renderSwitch({
    core: createFakeCore({ tailnetAvailable: true }),
  });
  expect(
    await act(() =>
      view.result.current.saveControlUrlAndSwitch('hs.example.com/'),
    ),
  ).toBe('switched');
  expect(view.settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
  expect(view.settingsStore.get('tailnet.mode')).toBe('embedded');
  const calls = [...view.core.calls];
  expect(
    await act(() => view.result.current.saveControlUrlAndSwitch('ftp://x')),
  ).toBe('invalid');
  expect(view.core.calls).toEqual(calls);
  expect(view.settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
});

it('opens an existing login URL without requesting a new login', async () => {
  const view = await renderSwitch({
    core: createFakeCore({
      tailnetAvailable: true,
      tailnet: tailnetNeedsLogin,
    }),
  });
  await act(() => view.result.current.openLogin());
  expect(view.platform.opened).toEqual([
    'https://hs.example.com/register/nodekey-test',
  ]);
  expect(view.core.calls.some(call => call.method === 'tailnet.login')).toBe(
    false,
  );
});

it('starts a login window when no URL is available', async () => {
  const view = await renderSwitch({
    core: createFakeCore({
      tailnetAvailable: true,
      tailnet: { ...tailnetNeedsLogin, authUrl: null },
    }),
  });
  await act(() => view.result.current.openLogin());
  expect(view.platform.opened).toEqual([]);
  expect(view.core.calls.some(call => call.method === 'tailnet.login')).toBe(
    true,
  );
  expect(tailnetSwitchStore.getState().loginWindowUntilMs).toBe(
    Date.now() + LOGIN_WINDOW_MS,
  );
});

it('reports status, browser, and login errors as not opened', async () => {
  const view = await renderSwitch({
    core: createFakeCore({
      tailnetAvailable: true,
      tailnet: tailnetNeedsLogin,
    }),
  });
  view.core.failNext('tailnet.status', new Error('status failed'));
  await act(async () => {
    await expect(view.result.current.openLogin()).resolves.toBe(false);
  });
  jest.spyOn(view.platform, 'openUrl').mockRejectedValue(new Error('failed'));
  await act(async () => {
    await expect(view.result.current.openLogin()).resolves.toBe(false);
  });
  await act(() =>
    view.core.setTailnet({ status: { ...tailnetNeedsLogin, authUrl: null } }),
  );
  view.core.failNext('tailnet.login', new Error('login failed'));
  await act(async () => {
    await expect(view.result.current.openLogin()).resolves.toBe(false);
  });
});
