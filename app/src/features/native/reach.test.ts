import { act, cleanup, renderHook } from '@testing-library/react-native';
import { createElement } from 'react';
import { renderWithProviders } from '../../test/render';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { ComputerEntry } from './types';
import { useManualReach } from './reach';
import type { ManualReach } from './reach';

function manualEntry(
  id: string,
  os: 'macos' | 'windows' = 'macos',
): ComputerEntry {
  return {
    kind: 'manual',
    key: `manual:${id}`,
    manual: {
      id,
      kind: 'computer',
      host: `192.168.31.${id === 'one' ? '50' : '51'}`,
      port: os === 'macos' ? 5900 : 3389,
      hostId: null,
      name: id,
      networkId: null,
      addedAtUs: 1,
      os,
    },
  };
}

async function renderReach(
  entries: ComputerEntry[],
  visible = true,
  core: FakeCore = createFakeCore(),
) {
  const providers = await renderWithProviders(createElement('View'), { core });
  await providers.unmount();
  const rendered = await renderHook<ManualReach, { shown: boolean }>(
    ({ shown }) => useManualReach(entries, shown),
    {
      initialProps: { shown: visible },
      wrapper: providers.wrapper,
    },
  );
  return { ...rendered, core };
}

async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
  jest.restoreAllMocks();
});

it('probes visible manual entries immediately and again every 30 seconds', async () => {
  const core = createFakeCore();
  const probe = jest
    .spyOn(core.discover!, 'probe')
    .mockResolvedValue({ addr: 'x', port: 5900, outcome: 'refused', ms: 1 });
  const rendered = await renderReach([manualEntry('one')], true, core);
  await flush();

  expect(probe).toHaveBeenCalledTimes(1);
  expect(rendered.result.current['manual:one']).toBe('online');
  await act(async () => {
    await jest.advanceTimersByTimeAsync(30_000);
  });
  expect(probe).toHaveBeenCalledTimes(2);
});

it('uses the OS protocol and never exceeds eight concurrent probes', async () => {
  const core = createFakeCore();
  let active = 0;
  let peak = 0;
  const pending: Array<() => void> = [];
  const probe = jest
    .spyOn(core.discover!, 'probe')
    .mockImplementation(async () => {
      active += 1;
      peak = Math.max(peak, active);
      await new Promise<void>(resolve => pending.push(resolve));
      active -= 1;
      return { addr: 'x', port: 3389, outcome: 'open', ms: 1 };
    });
  const entries = Array.from({ length: 10 }, (_, index) =>
    manualEntry(index === 0 ? 'one' : `m${index}`, 'windows'),
  );
  await renderReach(entries, true, core);
  await flush();
  expect(peak).toBe(8);
  while (probe.mock.calls.length < entries.length) {
    await act(async () => {
      pending.splice(0).forEach(resolve => resolve());
      await Promise.resolve();
    });
  }
  expect(probe.mock.calls).toContainEqual(['192.168.31.50', 3389, 'rdp', 800]);
  expect(probe).toHaveBeenCalledTimes(10);
});

it('does not probe while invisible and reports unknown without discover', async () => {
  const core = createFakeCore();
  const probe = jest.spyOn(core.discover!, 'probe');
  const rendered = await renderReach([manualEntry('one')], false, core);
  await flush();
  expect(probe).not.toHaveBeenCalled();
  expect(rendered.result.current['manual:one']).toBe('unknown');

  core.discover = undefined;
  await rendered.rerender({ shown: true });
  await flush();
  expect(rendered.result.current['manual:one']).toBe('unknown');
});
