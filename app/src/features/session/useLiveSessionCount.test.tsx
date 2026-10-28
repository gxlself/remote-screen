import { act, cleanup, renderHook } from '@testing-library/react-native';
import type { ReactNode } from 'react';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { adapterLive, resetAdapterLive } from '../adapters/liveStore';
import type { CollapsedAdapter } from '../adapters/liveStore';
import HistoryRecorder from '../sessions/HistoryRecorder';
import { resetSshStore } from '../sessions/sshStore';
import { renderWithProviders } from '../../test/render';
import {
  countAdapterSessions,
  resetAdapterPages,
  useAdapterPageLive,
  useLiveSessionCount,
} from './useLiveSessionCount';

beforeEach(() => {
  resetSshStore();
  resetAdapterLive();
  resetAdapterPages();
});

function collapsed(
  sessionId: string,
  phase: CollapsedAdapter['phase'] = 'streaming',
): CollapsedAdapter {
  return {
    sessionId,
    params: { target: { protocol: 'vnc', addr: '10.0.0.7' } },
    name: 'Mac mini',
    phase,
    error: null,
    screens: [],
    screenIndex: 0,
    mode: 'trackpad',
    // Folded away just now: the keeper closes records left too long.
    startedAtUs: Date.now() * 1000,
    collapsedAtUs: Date.now() * 1000,
    history: {
      live: null,
      stoppedAtUs: null,
      error: null,
      name: 'Mac mini',
      targetId: 'addr:vnc:10.0.0.7',
    },
  };
}

afterEach(async () => {
  await cleanup();
});

it('counts live remote sessions and open SSH sessions', async () => {
  const core = createFakeCore();
  const providers = await renderWithProviders(<HistoryRecorder />, { core });
  const { result } = await renderHook(() => useLiveSessionCount(), {
    wrapper: providers.wrapper as ({
      children,
    }: {
      children: ReactNode;
    }) => ReactNode,
  });

  expect(result.current).toBe(1);

  await act(() =>
    core.emit({
      type: 'ssh.state',
      sshId: 'ssh-1',
      serverId: 'gpu-train-02',
      state: 'open',
    }),
  );
  await act(() =>
    core.emit({
      type: 'ssh.state',
      sshId: 'ssh-2',
      serverId: 'prod-api-01',
      state: 'open',
    }),
  );
  expect(result.current).toBe(3);

  await act(() =>
    core.emit({
      type: 'ssh.state',
      sshId: 'ssh-2',
      serverId: 'prod-api-01',
      state: 'closed',
    }),
  );
  expect(result.current).toBe(2);
});

it('counts each adapter page once, with or without an id yet', () => {
  expect(countAdapterSessions({}, [])).toBe(0);
  expect(countAdapterSessions({ p1: null, p2: 'a-1' }, [])).toBe(2);
  // A folded-away session its page is taking back is the same session.
  expect(countAdapterSessions({ p1: 'a-1' }, ['a-1', 'a-2'])).toBe(2);
});

it('counts open adapter pages and folded-away adapter sessions', async () => {
  const core = createFakeCore();
  const providers = await renderWithProviders(<HistoryRecorder />, { core });
  const wrapper = providers.wrapper as ({
    children,
  }: {
    children: ReactNode;
  }) => ReactNode;
  const { result } = await renderHook(() => useLiveSessionCount(), {
    wrapper,
  });
  const base = result.current;

  await act(() => adapterLive.put(collapsed('a-1')));
  expect(result.current).toBe(base + 1);

  // Dropped while away: shown on Home, but nothing is kept alive for it.
  await act(() => adapterLive.put(collapsed('a-2', 'ended')));
  expect(result.current).toBe(base + 1);

  const page = await renderHook(
    ({ id, live }: { id: string | null; live: boolean }) =>
      useAdapterPageLive(id, live),
    { initialProps: { id: null as string | null, live: true }, wrapper },
  );
  expect(result.current).toBe(base + 2);

  // The page picks the folded-away session back up.
  await page.rerender({ id: 'a-1', live: true });
  expect(result.current).toBe(base + 1);
  await act(() => {
    adapterLive.take('a-1');
  });
  expect(result.current).toBe(base + 1);

  await page.rerender({ id: 'a-1', live: false });
  expect(result.current).toBe(base);

  await page.rerender({ id: 'a-3', live: true });
  expect(result.current).toBe(base + 1);
  await page.unmount();
  expect(result.current).toBe(base);
});
