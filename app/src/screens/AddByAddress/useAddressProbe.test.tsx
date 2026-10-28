import { act, cleanup, renderHook } from '@testing-library/react-native';
import { createElement } from 'react';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import type {
  CoreEventOf,
  DiscoverOutcome,
  DiscoverProbe,
} from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { NATIVE_PROBE_TIMEOUT_MS, useAddressProbe } from './useAddressProbe';

const HOST = '10.30.99.9';

function rsResult(
  outcome: CoreEventOf<'probe.result'>['outcome'],
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId: 'p1',
    hostId: null,
    path: 'intranet',
    addr: HOST,
    port: 7400,
    transport: 'tcp',
    outcome,
    ms: 21,
    last: true,
  };
}

function discoverOutcomes(
  core: FakeCore,
  outcomes: Partial<Record<number, DiscoverOutcome>>,
) {
  return jest
    .spyOn(core.discover!, 'probe')
    .mockImplementation(async (addr, port) => ({
      addr,
      port,
      outcome: outcomes[port] ?? 'timeout',
      ms: 4,
    }));
}

function rspDials(core: FakeCore) {
  return core.calls
    .filter(call => call.method === 'diag.probeAddress')
    .map(call => call.args);
}

async function flush() {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
}

async function mounted(core: FakeCore) {
  const providers = await renderWithProviders(createElement('View'), {
    core,
  });
  await providers.unmount();
  return renderHook(() => useAddressProbe(), { wrapper: providers.wrapper });
}

async function find(
  view: Awaited<ReturnType<typeof mounted>>,
  host = HOST,
  port = 7400,
) {
  await act(async () => {
    view.result.current.start(host, port);
  });
  await flush();
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('probes 22 with a TCP connect and dials RSP only for the Host port', async () => {
  const core = createFakeCore();
  const probe = discoverOutcomes(core, {});
  const view = await mounted(core);
  await find(view);

  expect(rspDials(core)).toEqual([[HOST, 7400]]);
  expect(probe.mock.calls).toEqual([
    [HOST, 3389, 'rdp', NATIVE_PROBE_TIMEOUT_MS],
    [HOST, 5900, 'vnc', NATIVE_PROBE_TIMEOUT_MS],
    [HOST, 22, 'tcp', NATIVE_PROBE_TIMEOUT_MS],
  ]);
});

it.each([
  ['open', 'open'],
  ['refused', 'closed'],
  ['timeout', 'closed'],
  ['mismatch', 'closed'],
] as const)('maps TCP outcome %s on 22 to %s', async (outcome, ssh) => {
  const core = createFakeCore();
  discoverOutcomes(core, { 22: outcome });
  const view = await mounted(core);
  await find(view);
  await act(async () => core.emit(rsResult('refused')));

  expect(view.result.current.probe).toMatchObject({ state: 'done', ssh });
});

it('treats a rejected TCP probe as closed', async () => {
  const core = createFakeCore();
  jest
    .spyOn(core.discover!, 'probe')
    .mockRejectedValue(new RsCoreError('network', 'x'));
  const view = await mounted(core);
  await find(view);
  await act(async () => core.emit(rsResult('ok')));

  expect(view.result.current.probe).toMatchObject({
    state: 'done',
    ssh: 'closed',
  });
});

it('waits for the TCP probe on 22 when probeAddress is unsupported', async () => {
  const core = createFakeCore();
  let resolveSsh: (result: DiscoverProbe) => void = () => {};
  jest
    .spyOn(core.discover!, 'probe')
    .mockImplementation((addr, port) =>
      port === 22
        ? new Promise(done => {
            resolveSsh = done;
          })
        : Promise.resolve({ addr, port, outcome: 'timeout', ms: 4 }),
    );
  core.failNext('diag.probeAddress', new RsCoreError('unsupported', 'x'));
  const view = await mounted(core);
  await find(view);

  expect(view.result.current.probe).toMatchObject({
    state: 'running',
    ssh: null,
  });
  await act(async () =>
    resolveSsh({ addr: HOST, port: 22, outcome: 'open', ms: 4 }),
  );
  expect(view.result.current.probe).toMatchObject({
    state: 'done',
    ssh: 'open',
  });
  expect(view.result.current.probe.rs?.reason).toBe('unsupported');
});

it('keeps the RSP dial for 3389 and 22 without discover', async () => {
  const core = createFakeCore();
  core.discover = undefined;
  const view = await mounted(core);
  await find(view);

  expect(rspDials(core)).toEqual([
    [HOST, 7400],
    [HOST, 3389],
    [HOST, 22],
  ]);
});
