import { act } from '@testing-library/react-native';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { computers, liveSession } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import ConnectingOverlay from './ConnectingOverlay';
import ApprovalOverlay from './ApprovalOverlay';
import ReconnectOverlay from './ReconnectOverlay';
import IdleOverlay from './IdleOverlay';
import LocalUseBar from './LocalUseBar';
import SharedOverlay from './SharedOverlay';
import PathNotice from './PathNotice';
import type { SessionCtx } from './slots';
import { hit } from '../../theme/tokens';

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: computers[0].hostId,
    hostName: computers[0].name,
    os: 'macos',
    host: computers[0],
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: {
      ...liveSession,
      phase: 'connecting',
      role: 'controller',
      controller: null,
      viewers: [],
      inputPausedLocal: false,
      stats: null,
      displays: [],
      dial: [],
      approvalTimeoutS: 30,
      derpRegion: null,
      reconnectReason: null,
      idleWarning: null,
      controlRequest: null,
      lastError: null,
      clipboard: [],
    },
    view: {
      overlay: 'connecting',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: false,
      shared: null,
      localUse: false,
      relay: false,
      tcp: false,
      idle: false,
      toolbar: {
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '自动',
        qualityA11y: '画质自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '',
    },
    isPad: false,
    reduceMotion: true,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: null,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect: jest.fn(),
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
    ...overrides,
  };
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(0);
});

afterEach(() => {
  jest.clearAllTimers();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('focuses each overlay title when it appears', async () => {
  const send = jest.spyOn(AccessibilityInfo, 'sendAccessibilityEvent');
  const cases = [
    <ConnectingOverlay ctx={context()} />,
    <ApprovalOverlay
      ctx={context({ view: { ...context().view, overlay: 'approval' } })}
    />,
    <ReconnectOverlay
      ctx={context({
        view: { ...context().view, overlay: 'reconnecting' },
        rec: { ...context().rec!, phase: 'reconnecting' },
        phaseAtMs: { reconnecting: 0 },
      })}
    />,
    <IdleOverlay
      ctx={context({
        view: { ...context().view, overlay: 'none', idle: true },
        rec: {
          ...context().rec!,
          phase: 'streaming',
          idleWarning: { secondsLeft: 30, lockOnEnd: false, atUs: 1 },
        },
      })}
    />,
  ];

  for (const item of cases) {
    const view = await renderWithProviders(item);
    await act(async () => {
      jest.advanceTimersByTime(0);
    });
    await act(async () => {
      await view.unmount();
    });
  }

  expect(send).toHaveBeenCalledTimes(4);
  expect(send.mock.calls.every(([, event]) => event === 'focus')).toBe(true);
});

it('announces a stuck step once without putting elapsed seconds in its label', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const base = context();
  const view = await renderWithProviders(
    <ConnectingOverlay
      ctx={context({
        rec: { ...base.rec!, phase: 'streaming' },
        phaseAtMs: { authenticating: 0, streaming: 700 },
      })}
    />,
  );

  await act(async () => {
    await jest.advanceTimersByTimeAsync(8400);
  });
  const stuck = view.getByTestId('connect-stuck');
  expect(stuck.props.accessibilityLiveRegion).toBeUndefined();
  expect(stuck.props.accessibilityLabel).not.toContain('秒');
  expect(announce).toHaveBeenCalledTimes(1);

  await act(async () => {
    await jest.advanceTimersByTimeAsync(1000);
  });
  expect(announce).toHaveBeenCalledTimes(1);
  await act(async () => {
    await view.unmount();
  });
});

it('uses a vertical step list and a scrolling panel at large font scale', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await renderWithProviders(<ConnectingOverlay ctx={context()} />);
  const steps = StyleSheet.flatten(
    view.getByTestId('connect-steps').props.style,
  );
  expect(steps).toMatchObject({
    flexDirection: 'column',
    alignItems: 'stretch',
  });
  expect(view.getByTestId('connecting-scroll')).toBeOnTheScreen();
  expect(view.getByLabelText('取消')).toBeOnTheScreen();
  await act(async () => {
    await view.unmount();
  });
});

it('does not announce LocalUseBar on Android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  announce.mockClear();
  const view = await renderWithProviders(
    <LocalUseBar
      ctx={context({ view: { ...context().view, localUse: true } })}
    />,
  );
  expect(announce).not.toHaveBeenCalled();
  await act(async () => {
    await view.unmount();
  });
});

it('separates handoff and path notice buttons from their live text', async () => {
  const core = createFakeCore({ sessions: [] });
  const first = context({
    myDeviceId: 'phone',
    rec: {
      ...context().rec!,
      role: 'viewer',
      controller: {
        deviceId: 'pad',
        deviceName: '小明的 iPad',
        user: 'me@example.com',
        path: 'tailnetDirect',
        since: 1,
      },
    },
  });
  const view = await renderWithProviders(<SharedOverlay ctx={first} />, {
    core,
  });
  await view.rerender(
    <SharedOverlay
      ctx={context({
        myDeviceId: 'phone',
        rec: { ...first.rec!, role: 'controller', controller: null },
      })}
    />,
  );
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  const handoff = view.getByTestId('shared-handoff');
  expect(handoff.props.accessibilityLabel).toBeUndefined();
  expect(
    view.getByLabelText(
      '已从 iPad 接管，现在由你控制。iPad 现在只能观看，它那边也收到了提示',
    ),
  ).toHaveProp('accessibilityLiveRegion', 'polite');
  expect(view.getByLabelText('知道了')).toBeOnTheScreen();
  expect(
    StyleSheet.flatten(view.getByLabelText('知道了').props.style),
  ).toMatchObject({
    minHeight: hit.min,
  });
  await act(async () => {
    await view.unmount();
  });

  const pathView = await renderWithProviders(
    <PathNotice ctx={context({ view: { ...context().view, relay: true } })} />,
  );
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  const path = pathView.getByTestId('path-notice');
  expect(path.props.accessibilityLabel).toBeUndefined();
  expect(pathView.getByLabelText('知道了')).toBeOnTheScreen();
  await act(async () => {
    await pathView.unmount();
  });
});

it('announces watcher changes on iOS and keeps text actions at the hit minimum on Android', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  announce.mockClear();
  const first = context({
    rec: { ...context().rec!, role: 'controller', viewers: [] },
  });
  const view = await renderWithProviders(<SharedOverlay ctx={first} />);
  await view.rerender(
    <SharedOverlay
      ctx={context({
        rec: {
          ...first.rec!,
          role: 'controller',
          viewers: [
            {
              deviceId: 'watcher',
              deviceName: '平板',
              user: 'friend@example.com',
              path: 'tailnetDirect',
              since: 1,
            },
          ],
        },
      })}
    />,
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(announce).toHaveBeenCalledWith('平板 在观看');
  await act(async () => {
    await view.unmount();
  });

  jest.replaceProperty(Platform, 'OS', 'android');
  const pathView = await renderWithProviders(
    <PathNotice ctx={context({ view: { ...context().view, relay: true } })} />,
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(
    StyleSheet.flatten(pathView.getByLabelText('知道了').props.style),
  ).toMatchObject({ minHeight: hit.min });
  await act(async () => {
    await pathView.unmount();
  });
});
