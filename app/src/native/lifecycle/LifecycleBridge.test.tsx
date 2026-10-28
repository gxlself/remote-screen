import type { ReactNode } from 'react';
import { act, render, waitFor } from '@testing-library/react-native';
import { CoreProvider } from '../rscore/CoreContext';
import { createFakeCore } from '../rscore/fakeCore';
import type { PowerState } from '../specs/NativeRsLifecycle';
import LifecycleBridge from './LifecycleBridge';
import { usePowerState } from './powerStore';

const mockSetIdleTimerDisabled = jest.fn<void, [boolean]>();
const mockSetBackgroundKeepAlive = jest.fn<void, [boolean]>();
const mockGetConstants = jest.fn<
  { previousRunEndedInBackground: boolean },
  []
>();
const mockGetPowerState = jest.fn<Promise<PowerState>, []>();
let mockPowerChange: ((state: PowerState) => void) | undefined;
let mockRemovePowerChange = jest.fn<void, []>();
let mockNativeModule: {
  getConstants: typeof mockGetConstants;
  setIdleTimerDisabled: typeof mockSetIdleTimerDisabled;
  setBackgroundKeepAlive: typeof mockSetBackgroundKeepAlive;
  getPowerState: typeof mockGetPowerState;
  onPowerChange: (listener: (state: PowerState) => void) => { remove: () => void };
} | null = null;
let mockLiveCount = 0;

jest.mock('../specs/NativeRsLifecycle', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

jest.mock('../../features/session/useLiveSessionCount', () => ({
  useLiveSessionCount: () => mockLiveCount,
}));

function wrapper(core: ReturnType<typeof createFakeCore>) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>{children}</CoreProvider>
  );
}

const power: PowerState = {
  lowPower: false,
  lowPowerMode: false,
  batteryLevel: 0.5,
  charging: false,
};

beforeEach(() => {
  mockLiveCount = 0;
  mockSetIdleTimerDisabled.mockClear();
  mockSetBackgroundKeepAlive.mockClear();
  mockGetConstants.mockReset().mockReturnValue({
    previousRunEndedInBackground: false,
  });
  mockGetPowerState.mockReset().mockResolvedValue(power);
  mockPowerChange = undefined;
  mockRemovePowerChange = jest.fn();
  mockNativeModule = {
    getConstants: mockGetConstants,
    setIdleTimerDisabled: mockSetIdleTimerDisabled,
    setBackgroundKeepAlive: mockSetBackgroundKeepAlive,
    getPowerState: mockGetPowerState,
    onPowerChange: listener => {
      mockPowerChange = listener;
      return { remove: mockRemovePowerChange };
    },
  };
  usePowerState.setState({ power, setPower: usePowerState.getState().setPower });
});

afterEach(() => {
  mockNativeModule = null;
});

it('tracks live sessions and clears lifecycle controls on unmount', async () => {
  const core = createFakeCore();
  const rendered = await render(<LifecycleBridge />, { wrapper: wrapper(core) });
  expect(mockSetIdleTimerDisabled).toHaveBeenCalledWith(false);
  expect(mockSetBackgroundKeepAlive).toHaveBeenCalledWith(false);

  mockLiveCount = 1;
  await act(() => rendered.rerender(<LifecycleBridge />));
  expect(mockSetIdleTimerDisabled).toHaveBeenCalledWith(true);
  expect(mockSetBackgroundKeepAlive).toHaveBeenCalledWith(true);

  await rendered.unmount();
  expect(mockSetIdleTimerDisabled).toHaveBeenLastCalledWith(false);
  expect(mockSetBackgroundKeepAlive).toHaveBeenLastCalledWith(false);
});

it('stores power changes and writes a low-power log', async () => {
  const core = createFakeCore();
  const rendered = await render(<LifecycleBridge />, { wrapper: wrapper(core) });
  await waitFor(() => expect(mockPowerChange).toBeDefined());
  const next: PowerState = { ...power, lowPower: true };
  await act(() => mockPowerChange?.(next));
  expect(usePowerState.getState().power).toEqual(next);
  expect(
    core.calls.some(
      call =>
        call.method === 'logs.write' &&
        call.args[1] === 'lifecycle' &&
        call.args[2] === 'lowPower=true',
    ),
  ).toBe(true);
  await rendered.unmount();
});

it('mounts safely when the native module is unavailable', async () => {
  mockNativeModule = null;
  const core = createFakeCore();
  const rendered = await render(<LifecycleBridge />, { wrapper: wrapper(core) });
  await rendered.unmount();
});
