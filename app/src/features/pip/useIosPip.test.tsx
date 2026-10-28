import { act, cleanup } from '@testing-library/react-native';
import { AppState, Platform } from 'react-native';
import type { ReactElement } from 'react';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import { setVideoStats, clearVideo } from '../session/videoStore';
import type { SessionController } from '../../screens/Session/useSessionController';
import type { SessionId } from '../../native/rscore/types';
import { getPipState, resetPipStateForTests } from './pipState';
import { useIosPip } from './useIosPip';

const mockPipSupported = jest.fn(() => true);
const mockSetPipEligibility = jest.fn();
let mockPipChangeListener:
  | ((event: { sessionId: string; active: boolean; reason: string }) => void)
  | undefined;
let mockPipPlaybackListener:
  | ((event: { sessionId: string; paused: boolean }) => void)
  | undefined;
const mockRemovePipChange = jest.fn();
const mockRemovePipPlayback = jest.fn();

jest.mock('./nativePip', () => ({
  pipSupported: () => mockPipSupported(),
  setPipEligibility: (...args: unknown[]) => mockSetPipEligibility(...args),
  subscribePipChange: (listener: typeof mockPipChangeListener) => {
    mockPipChangeListener = listener;
    return mockRemovePipChange;
  },
  subscribePipPlayback: (listener: typeof mockPipPlaybackListener) => {
    mockPipPlaybackListener = listener;
    return mockRemovePipPlayback;
  },
}));

function Harness({ ctx }: { ctx: SessionController }): ReactElement | null {
  useIosPip(ctx);
  return null;
}

function context(
  overrides: Partial<SessionController> = {},
): SessionController {
  return {
    sessionId: 's1' as SessionId,
    view: { role: 'viewer', overlay: 'none' } as SessionController['view'],
    rec: { phase: 'streaming' } as SessionController['rec'],
    showToast: jest.fn(),
    ...overrides,
  } as SessionController;
}

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'active',
  });
  mockPipSupported.mockClear();
  mockSetPipEligibility.mockClear();
  mockRemovePipChange.mockClear();
  mockRemovePipPlayback.mockClear();
  mockPipChangeListener = undefined;
  mockPipPlaybackListener = undefined;
  resetPipStateForTests();
  setVideoStats('s1' as SessionId, {
    fps: 60,
    decodeP50Ms: 1,
    decodeP95Ms: 2,
    latencyP50Ms: 3,
    queue: 0,
    dropped: 0,
    keyframeRequests: 0,
    width: 1920,
    height: 1080,
    codec: 'h264',
  });
});

afterEach(async () => {
  clearVideo('s1' as SessionId);
  await cleanup();
  jest.restoreAllMocks();
});

it('does not use iOS PiP APIs on Android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const core = createFakeCore();
  const view = await renderWithProviders(<Harness ctx={context()} />, { core });
  expect(mockSetPipEligibility).not.toHaveBeenCalled();
  expect(mockPipChangeListener).toBeUndefined();
  expect(mockPipPlaybackListener).toBeUndefined();
  await view.unmount();
});

it('syncs eligibility and avoids duplicate values', async () => {
  const core = createFakeCore();
  const view = await renderWithProviders(<Harness ctx={context()} />, { core });
  expect(mockSetPipEligibility).toHaveBeenCalledWith('s1', true, true);
  mockSetPipEligibility.mockClear();
  await act(() => view.rerender(<Harness ctx={context()} />));
  expect(mockSetPipEligibility).not.toHaveBeenCalled();
  await act(() =>
    view.rerender(
      <Harness
        ctx={context({
          view: {
            role: 'controller',
            overlay: 'none',
          } as SessionController['view'],
        })}
      />,
    ),
  );
  expect(mockSetPipEligibility).toHaveBeenCalledWith('s1', false, false);
  await view.unmount();
  expect(mockSetPipEligibility).toHaveBeenLastCalledWith(null, false, false);
});

it('maps playback events to the matching session', async () => {
  const core = createFakeCore();
  const view = await renderWithProviders(<Harness ctx={context()} />, { core });
  await act(() => mockPipPlaybackListener?.({ sessionId: 's1', paused: true }));
  await act(() =>
    mockPipPlaybackListener?.({ sessionId: 'other', paused: true }),
  );
  expect(
    core.calls.filter(call => call.method === 'sessions.background'),
  ).toEqual([{ method: 'sessions.background', args: ['s1', true] }]);
  await view.unmount();
});

it('shows a foreground toast for a PiP failure and tracks changes', async () => {
  const ctx = context();
  const core = createFakeCore();
  const view = await renderWithProviders(<Harness ctx={ctx} />, { core });
  await act(() =>
    mockPipChangeListener?.({
      sessionId: 's1',
      active: false,
      reason: 'notPossible',
    }),
  );
  expect(ctx.showToast).toHaveBeenCalledWith('暂时无法进入画中画');
  await act(() =>
    mockPipChangeListener?.({
      sessionId: 's1',
      active: true,
      reason: 'started',
    }),
  );
  expect(getPipState()).toEqual({ active: true, sessionId: 's1' });
  Object.defineProperty(AppState, 'currentState', {
    configurable: true,
    writable: true,
    value: 'background',
  });
  await act(() =>
    mockPipChangeListener?.({
      sessionId: 's1',
      active: false,
      reason: 'failed',
    }),
  );
  expect(ctx.showToast).toHaveBeenCalledTimes(1);
  await view.unmount();
});
