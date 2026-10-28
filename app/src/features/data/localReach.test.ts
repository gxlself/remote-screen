import { act, cleanup, renderHook } from '@testing-library/react-native';
import { focusManager } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import { createElement } from 'react';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { CoreEventOf } from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { MemoryKv } from '../storage/kv';
import {
  LOCAL_REACH_KEY,
  REACH_CONCURRENCY,
  REACH_INTERVAL_MS,
  REACH_TIMEOUT_MS,
  probeReach,
  readLocalReach,
  useLocalReach,
} from './localReach';

const clients: QueryClient[] = [];

function result(
  probeId: string,
  hostId: string,
  outcome: CoreEventOf<'probe.result'>['outcome'],
  last: boolean,
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId,
    hostId,
    path: 'tailnetDirect',
    addr: '100.64.0.7',
    port: 7400,
    transport: 'quic',
    outcome,
    ms: outcome === 'ok' ? 12 : null,
    last,
  };
}

function probeCalls(core: FakeCore): string[] {
  return core.calls
    .filter(call => call.method === 'diag.probe')
    .map(call => call.args[0] as string);
}

function hostIds(count: number): string[] {
  return Array.from({ length: count }, (_, index) => `host-${index + 10}`);
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function renderReach(
  ids: string[],
  visible: boolean,
  opts: { core?: FakeCore; kv?: MemoryKv } = {},
) {
  const core = opts.core ?? createFakeCore();
  const kv = opts.kv ?? new MemoryKv();
  const providers = await renderWithProviders(createElement('View'), {
    core,
    kv,
  });
  clients.push(providers.queryClient);
  await providers.unmount();
  const rendered = await renderHook(
    ({ shown }: { shown: boolean }) => useLocalReach(ids, shown),
    { initialProps: { shown: visible }, wrapper: providers.wrapper },
  );
  return { ...rendered, core, kv };
}

beforeEach(() => {
  jest.useFakeTimers();
  focusManager.setFocused(true);
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  focusManager.setFocused(undefined);
  jest.useRealTimers();
});

describe('probeReach', () => {
  it(`keeps at most ${REACH_CONCURRENCY} probes in flight`, async () => {
    const core = createFakeCore();
    const ids = hostIds(REACH_CONCURRENCY + 3);
    const round = probeReach(core, ids, () => 7);
    await advance();
    expect(probeCalls(core)).toEqual(ids.slice(0, REACH_CONCURRENCY));

    await advance(REACH_TIMEOUT_MS - 10);
    expect(probeCalls(core)).toHaveLength(REACH_CONCURRENCY);
    await advance(20);
    expect(probeCalls(core)).toEqual(ids);

    await advance(REACH_TIMEOUT_MS);
    const reach = await round;
    expect(Object.keys(reach).sort()).toEqual([...ids].sort());
    expect(reach[ids[0] as string]).toEqual({ online: false, at: 7 });
  });

  it('frees a slot as soon as a probe finishes', async () => {
    const core = createFakeCore();
    const ids = hostIds(REACH_CONCURRENCY + 1);
    const round = probeReach(core, ids);
    await advance();
    expect(probeCalls(core)).toHaveLength(REACH_CONCURRENCY);
    await act(() => core.emit(result('p1', ids[0] as string, 'ok', false)));
    await advance();
    expect(probeCalls(core)).toEqual(ids);
    await advance(REACH_TIMEOUT_MS);
    await round;
  });

  it('counts ok as online and a timeout, a failure or a last miss as offline', async () => {
    const core = createFakeCore();
    jest.spyOn(core.diag, 'probe').mockImplementation(async hostId => {
      if (hostId === 'early') {
        // Emitted before the probe id resolves, as Rust does for NoAddress.
        core.emit(result('id-early', hostId, 'ok', true));
      }
      if (hostId === 'failing') {
        throw new Error('probe failed');
      }
      return `id-${hostId}`;
    });
    const round = probeReach(core, [
      'ok',
      'refused',
      'silent',
      'early',
      'failing',
    ]);
    await advance();
    await act(() => {
      core.emit(result('id-ok', 'ok', 'timeout', false));
      core.emit(result('id-ok', 'ok', 'ok', false));
      core.emit(result('id-refused', 'refused', 'refused', false));
      core.emit(result('id-refused', 'refused', 'fingerprintMismatch', true));
      core.emit(result('elsewhere', 'silent', 'ok', true));
    });
    await advance(REACH_TIMEOUT_MS);
    const reach = await round;
    expect(
      Object.fromEntries(
        Object.entries(reach).map(([hostId, value]) => [hostId, value.online]),
      ),
    ).toEqual({
      ok: true,
      refused: false,
      silent: false,
      early: true,
      failing: false,
    });
    expect(core.subscriberCount()).toBe(0);
  });

  it('times out a probe that never reports at the per-host limit', async () => {
    const core = createFakeCore();
    let settled = false;
    const round = probeReach(core, ['quiet']).then(reach => {
      settled = true;
      return reach;
    });
    await advance(REACH_TIMEOUT_MS - 10);
    expect(settled).toBe(false);
    await advance(20);
    expect(settled).toBe(true);
    expect((await round).quiet?.online).toBe(false);
  });

  it('runs rounds on one core one after another', async () => {
    const core = createFakeCore();
    const first = probeReach(core, hostIds(REACH_CONCURRENCY));
    const second = probeReach(core, ['later']);
    await advance();
    expect(probeCalls(core)).not.toContain('later');
    await advance(REACH_TIMEOUT_MS);
    await first;
    await advance();
    expect(probeCalls(core)).toContain('later');
    await advance(REACH_TIMEOUT_MS);
    await second;
  });
});

describe('useLocalReach', () => {
  it('does not probe while hidden', async () => {
    const { core, result: hook } = await renderReach(['a', 'b'], false);
    await advance(3 * REACH_INTERVAL_MS);
    expect(probeCalls(core)).toEqual([]);
    expect(hook.current.reach).toEqual({});
  });

  it('probes when shown, then every 30 s in the foreground only', async () => {
    const { core, rerender } = await renderReach(['a', 'b'], false);
    await rerender({ shown: true });
    await advance();
    expect(probeCalls(core)).toEqual(['a', 'b']);

    // react-query restarts the interval on every query update, so the next
    // round starts 30 s after the previous one finished.
    await advance(REACH_TIMEOUT_MS);
    await advance(REACH_INTERVAL_MS - 10);
    expect(probeCalls(core)).toHaveLength(2);
    await advance(20);
    await advance(10);
    expect(probeCalls(core)).toHaveLength(4);

    await act(() => focusManager.setFocused(false));
    await advance(3 * REACH_INTERVAL_MS);
    expect(probeCalls(core)).toHaveLength(4);

    await rerender({ shown: false });
    await act(() => focusManager.setFocused(true));
    await advance(3 * REACH_INTERVAL_MS);
    expect(probeCalls(core)).toHaveLength(4);
  });

  it('starts from the cached results and writes new ones back', async () => {
    const kv = new MemoryKv();
    kv.setJSON(LOCAL_REACH_KEY, {
      a: { online: true, at: 100 },
      gone: { online: true, at: 50 },
      bad: { online: 'yes', at: 1 },
    });
    const core = createFakeCore();
    jest.spyOn(core.diag, 'probe').mockImplementation(async hostId => {
      core.emit(result(`id-${hostId}`, hostId, 'ok', true));
      return `id-${hostId}`;
    });
    const { result: hook } = await renderReach(['a', 'b'], false, { core, kv });
    expect(hook.current.reach).toEqual({ a: { online: true, at: 100 } });

    await act(async () => {
      await hook.current.refresh();
    });
    await advance();
    expect(hook.current.reach.a?.online).toBe(true);
    expect(hook.current.reach.b?.online).toBe(true);
    expect(readLocalReach(kv)).toEqual({
      a: hook.current.reach.a,
      b: hook.current.reach.b,
      gone: { online: true, at: 50 },
    });
  });

  it('ignores a malformed cache', () => {
    const kv = new MemoryKv();
    for (const raw of [
      '{',
      'null',
      '[]',
      '{"a":null}',
      '{"a":{"online":true}}',
    ]) {
      kv.set(LOCAL_REACH_KEY, raw);
      expect(readLocalReach(kv)).toEqual({});
    }
  });
});
