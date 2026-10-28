import type { ReactNode } from 'react';
import { Platform } from 'react-native';
import { act, render } from '@testing-library/react-native';
import { CoreProvider } from '../../native/rscore/CoreContext';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { usePowerState } from '../../native/lifecycle/powerStore';
import PowerSync from './PowerSync';

function wrapper(core: FakeCore) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>{children}</CoreProvider>
  );
}

function setLowPower(lowPower: boolean): void {
  const power = usePowerState.getState().power;
  usePowerState.setState({
    power: {
      ...power,
      lowPower,
    },
  });
}

function syncCalls(core: FakeCore) {
  return core.calls.filter(call => call.method === 'settings.set');
}

beforeEach(() => {
  setLowPower(false);
});

afterEach(() => {
  jest.restoreAllMocks();
});

it('sends the low-power state on mount and on each change', async () => {
  const core = createFakeCore();
  const view = await render(<PowerSync />, { wrapper: wrapper(core) });

  expect(syncCalls(core)).toEqual([
    { method: 'settings.set', args: ['device.lowPower', false] },
  ]);

  await act(async () => setLowPower(true));
  expect(syncCalls(core)).toEqual([
    { method: 'settings.set', args: ['device.lowPower', false] },
    { method: 'settings.set', args: ['device.lowPower', true] },
  ]);

  await view.unmount();
});

it('warns once and stops syncing when the setting is unsupported', async () => {
  const warn = jest.spyOn(console, 'warn').mockImplementation(() => {});
  const core = createFakeCore();
  core.failNext('settings.set', new Error('unsupported'));
  const view = await render(<PowerSync />, { wrapper: wrapper(core) });

  expect(warn).toHaveBeenCalledTimes(1);
  expect(syncCalls(core)).toHaveLength(1);

  await act(async () => setLowPower(true));
  await act(async () => setLowPower(false));

  expect(warn).toHaveBeenCalledTimes(1);
  expect(syncCalls(core)).toHaveLength(1);
  await view.unmount();
});

it('sends Android background hold once per core', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const core = createFakeCore();
  const view = await render(<PowerSync />, { wrapper: wrapper(core) });

  expect(
    syncCalls(core).filter(call => call.args[0] === 'device.backgroundHold'),
  ).toEqual([
    { method: 'settings.set', args: ['device.backgroundHold', true] },
  ]);

  await act(async () => setLowPower(true));
  expect(
    syncCalls(core).filter(call => call.args[0] === 'device.backgroundHold'),
  ).toHaveLength(1);
  await view.unmount();
});
