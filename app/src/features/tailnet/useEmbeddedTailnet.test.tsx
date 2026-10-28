import { act, cleanup, renderHook } from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import {
  netCompanyVpnEmbedded,
  routeEmbeddedRunning,
  tailnetNeedsLogin,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { TailnetStatus } from '../../native/rscore/types';
import { useEmbeddedTailnet } from './useEmbeddedTailnet';

async function renderTailnet(
  core = createFakeCore(),
  opts?: { pollMs?: number },
  tailnetEnabled = true,
) {
  const providers = await renderWithProviders(<></>, {
    core,
    settings: { 'tailnet.enabled': tailnetEnabled },
  });
  await providers.unmount();
  const view = await renderHook(() => useEmbeddedTailnet(opts), {
    wrapper: providers.wrapper,
  });
  return { ...view, core };
}

function statusCalls(core: ReturnType<typeof createFakeCore>) {
  return core.calls.filter(call => call.method === 'tailnet.status');
}

beforeEach(() => jest.useFakeTimers());
afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('reports unavailable without requesting status', async () => {
  const view = await renderTailnet();
  expect(view.result.current.available).toBe(false);
  expect(view.result.current.status).toBeNull();
  expect(statusCalls(view.core)).toHaveLength(0);
  await act(() => view.result.current.refresh());
  expect(statusCalls(view.core)).toHaveLength(0);
});

it('asks the in-app tailnet nothing while Tailscale is switched off', async () => {
  const core = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetRunning,
  });
  const view = await renderTailnet(core, { pollMs: 1000 }, false);
  expect(view.result.current.available).toBe(false);
  expect(view.result.current.status).toBeNull();
  await act(() => jest.advanceTimersByTimeAsync(5000));
  await act(() => view.result.current.refresh());
  expect(core.calls.map(call => call.method)).not.toContain(
    'tailnet.available',
  );
  expect(statusCalls(core)).toHaveLength(0);
});

it('treats a failed availability check as unavailable', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  core.failNext('tailnet.available', new Error('unsupported'));
  const view = await renderTailnet(core);
  expect(view.result.current.available).toBe(false);
  expect(view.result.current.status).toBeNull();
  expect(statusCalls(core)).toHaveLength(0);
});

it('loads status, refreshes on events, and tracks route summaries', async () => {
  const core = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetNeedsLogin,
  });
  const view = await renderTailnet(core);
  expect(view.result.current.status?.state).toBe('needsLogin');
  expect(statusCalls(core)).toHaveLength(1);

  await act(() => core.setTailnet({ status: tailnetRunning }));
  expect(view.result.current.status?.state).toBe('running');
  expect(view.result.current.status?.peers).toHaveLength(2);
  expect(statusCalls(core)).toHaveLength(2);

  await act(() => core.setTailnet({ route: routeEmbeddedRunning }));
  expect(view.result.current.route?.active).toBe('embedded');
});

it('polls only while embedded is running and clears its interval', async () => {
  const core = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetRunning,
    network: netCompanyVpnEmbedded,
  });
  const view = await renderTailnet(core, { pollMs: 10_000 });
  expect(view.result.current.route?.active).toBe('embedded');
  const before = statusCalls(core).length;
  await act(() => jest.advanceTimersByTimeAsync(10_000));
  expect(statusCalls(core)).toHaveLength(before + 1);

  await act(() => core.setTailnet({ route: null }));
  await act(() => jest.advanceTimersByTimeAsync(20_000));
  expect(view.result.current.route).toBeNull();
  expect(statusCalls(core)).toHaveLength(before + 1);

  await act(() => core.setTailnet({ route: routeEmbeddedRunning }));
  await view.unmount();
  await act(() => jest.advanceTimersByTimeAsync(20_000));
  await act(() => view.result.current.refresh());
  expect(statusCalls(core)).toHaveLength(before + 1);
});

it('keeps the previous status after a failed refresh', async () => {
  const core = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetRunning,
  });
  const view = await renderTailnet(core);
  core.failNext('tailnet.status', new Error('status failed'));
  await act(() => core.setTailnet({ status: tailnetNeedsLogin }));
  expect(view.result.current.status?.state).toBe('running');
  await act(() => view.result.current.refresh());
  expect(view.result.current.status?.state).toBe('needsLogin');
});

it('does not overwrite newer status with an older response', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  let finish!: (status: TailnetStatus) => void;
  const pending = new Promise<TailnetStatus>(resolve => {
    finish = resolve;
  });
  jest.spyOn(core.tailnet, 'status').mockImplementationOnce(() => pending);
  const view = await renderTailnet(core);
  expect(view.result.current.status).toBeNull();
  await act(() => core.setTailnet({ status: tailnetRunning }));
  expect(view.result.current.status?.state).toBe('running');
  await act(() => finish(tailnetNeedsLogin));
  expect(view.result.current.status?.state).toBe('running');
});

it('discards an availability result arriving after unmount', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  let finish!: (available: boolean) => void;
  const pending = new Promise<boolean>(resolve => {
    finish = resolve;
  });
  jest.spyOn(core.tailnet, 'available').mockImplementationOnce(() => pending);
  const view = await renderTailnet(core);
  expect(view.result.current.available).toBeNull();
  await view.unmount();
  await act(() => finish(true));
  expect(statusCalls(core)).toHaveLength(0);
});
