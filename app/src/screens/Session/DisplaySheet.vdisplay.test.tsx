import { act, cleanup, fireEvent } from '@testing-library/react-native';
import { Dimensions, PixelRatio } from 'react-native';
import { useSessions } from '../../features/session/store';
import { resetVdisplayStore, useVdisplayStore } from '../../features/vdisplay';
import { vdisplay } from '../../i18n/zh/vdisplay';
import { HOST_FEATURE_VIRTUAL_DISPLAY } from '../../native/rscore/types';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { computers, liveSession } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import DisplaySheet from './DisplaySheet';
import type { SessionCtx } from './slots';

const displays = [
  {
    id: 1,
    name: 'Studio Display',
    widthPx: 2560,
    heightPx: 1600,
    primary: true,
    refreshHz: 60,
  },
  {
    id: 2,
    name: '虚拟屏（iPad）',
    widthPx: 2732,
    heightPx: 2048,
    primary: false,
    refreshHz: 60,
    isVirtual: true,
  },
];

function Harness({ ctx, onClose }: { ctx: SessionCtx; onClose: () => void }) {
  const rec = useSessions(state => state[liveSession.id]);
  return <DisplaySheet ctx={{ ...ctx, rec }} onClose={onClose} />;
}

function baseCtx(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: liveSession.hostId,
    hostName: liveSession.hostName,
    os: 'macos',
    host: {
      ...computers[0],
      features: HOST_FEATURE_VIRTUAL_DISPLAY,
    },
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: undefined,
    view: {
      overlay: 'none',
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
        qualityA11y: '画质：自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: 6,
      fps: 60,
      tcp: false,
      alertText: null,
      a11y: '连接质量：直连',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: 0,
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

function screenMocks() {
  jest.spyOn(Dimensions, 'get').mockReturnValue({
    width: 1024,
    height: 1366,
    scale: 2,
    fontScale: 2,
  });
  jest.spyOn(PixelRatio, 'get').mockReturnValue(2);
}

async function mount(
  options: {
    role?: 'controller' | 'viewer';
    host?: SessionCtx['host'];
    currentId?: number;
    own?: number;
  } = {},
) {
  screenMocks();
  const core = createFakeCore({
    sessions: [{ ...liveSession, role: options.role ?? 'controller' }],
  });
  const onClose = jest.fn();
  const showToast = jest.fn();
  if (options.own !== undefined) {
    useVdisplayStore.setState({ owned: { [liveSession.id]: options.own } });
  }
  const view = await renderWithProviders(
    <Harness
      ctx={baseCtx({
        host: options.host === undefined ? baseCtx().host : options.host,
        showToast,
      })}
      onClose={onClose}
    />,
    { core },
  );
  await act(async () => {
    core.emit({
      type: 'session.displays',
      sessionId: liveSession.id,
      displays,
      currentId: options.currentId ?? 1,
    });
  });
  return { ...view, core, onClose, showToast };
}

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
  jest.restoreAllMocks();
  resetVdisplayStore();
});

describe('DisplaySheet virtual display controls', () => {
  it('shows the create button for a controller with the feature', async () => {
    const view = await mount();
    expect(view.getByText('新建虚拟屏 · 2732×2048')).toBeOnTheScreen();
  });

  it('hides creation without the feature', async () => {
    const noFeature = await mount({
      host: { ...baseCtx().host!, features: 0 },
    });
    expect(noFeature.queryByTestId('display-vd-create')).toBeNull();
  });

  it('hides creation for a viewer', async () => {
    const viewer = await mount({ role: 'viewer' });
    expect(viewer.queryByTestId('display-vd-create')).toBeNull();
  });

  it('sends the request and displays pending state', async () => {
    const view = await mount();
    await fireEvent.press(view.getByTestId('display-vd-create'));
    expect(view.getByTestId('display-vd-pending')).toBeOnTheScreen();
    expect(view.core.calls).toContainEqual({
      method: 'sessions.createVirtualDisplay',
      args: [
        liveSession.id,
        {
          widthPx: 2732,
          heightPx: 2048,
          scale: 2,
          refreshHz: 60,
          select: true,
        },
      ],
    });
    expect(view.getByTestId('display-vd-pending')).toHaveTextContent(
      vdisplay.creating,
    );
    expect(view.getByTestId('display-sheet')).toBeOnTheScreen();
  });

  it('handles a matching result and remembers the owned display', async () => {
    const view = await mount();
    await fireEvent.press(view.getByTestId('display-vd-create'));
    expect(view.getByTestId('display-vd-pending')).toBeOnTheScreen();
    await act(async () => {
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: liveSession.id,
        requestId: 1,
        status: 'ok',
        display: { ...displays[1], id: 9 },
      });
    });
    expect(useVdisplayStore.getState().owned[liveSession.id]).toBe(9);
    expect(view.onClose).toHaveBeenCalledTimes(1);
  });

  it('ignores events for another request or session', async () => {
    const view = await mount();
    await fireEvent.press(view.getByTestId('display-vd-create'));
    await act(async () => {
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: liveSession.id,
        requestId: 99,
        status: 'ok',
        display: { ...displays[1], id: 9 },
      });
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: 'other-session',
        requestId: 1,
        status: 'ok',
        display: { ...displays[1], id: 9 },
      });
    });
    expect(view.getByTestId('display-vd-pending')).toBeOnTheScreen();
  });

  it('shows errors and handles timeout and unsupported binding', async () => {
    const view = await mount();
    await fireEvent.press(view.getByTestId('display-vd-create'));
    await act(async () => {
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: liveSession.id,
        requestId: 1,
        status: 'limit',
      });
    });
    expect(view.getByTestId('display-vd-error')).toHaveTextContent(
      vdisplay.error.limit,
    );

    view.core.sessions.createVirtualDisplay = jest.fn(() => {
      throw new RsCoreError('unsupported', 'x');
    });
    await fireEvent.press(view.getByTestId('display-vd-create'));
    expect(view.getByTestId('display-vd-error')).toHaveTextContent(
      vdisplay.error.unsupported,
    );

    jest.useFakeTimers();
    view.core.sessions.createVirtualDisplay = jest.fn(() => 2);
    await fireEvent.press(view.getByTestId('display-vd-create'));
    await act(async () => {
      jest.advanceTimersByTime(15_000);
    });
    expect(view.getByTestId('display-vd-error')).toHaveTextContent(
      vdisplay.error.timeout,
    );
  });

  it('marks virtual displays and removes the owned display after confirming', async () => {
    const view = await mount({ own: 2, currentId: 2 });
    expect(view.getByTestId('display-virtual-2')).toHaveTextContent(
      vdisplay.badge,
    );
    expect(view.getByTestId('display-vd-remove')).toHaveTextContent(
      vdisplay.remove,
    );
    await fireEvent.press(view.getByTestId('display-vd-remove'));
    expect(
      view.core.calls.some(
        call => call.method === 'sessions.releaseVirtualDisplay',
      ),
    ).toBe(false);
    await fireEvent.press(view.getByTestId('display-vd-remove-confirm'));
    expect(view.core.calls).toContainEqual({
      method: 'sessions.releaseVirtualDisplay',
      args: [liveSession.id, 2],
    });
    expect(useVdisplayStore.getState().owned[liveSession.id]).toBeUndefined();
    // 结果回来之前不说「已移除」。
    expect(view.getByTestId('display-vd-removing')).toHaveTextContent(
      vdisplay.removing,
    );
    expect(view.showToast).not.toHaveBeenCalled();
    expect(view.onClose).not.toHaveBeenCalled();

    await act(async () => {
      view.core.emit({
        type: 'session.displays',
        sessionId: liveSession.id,
        displays: [displays[0]],
        currentId: 1,
      });
    });
    expect(view.showToast).toHaveBeenCalledWith(vdisplay.removed, {
      tone: 'good',
    });
    expect(view.onClose).toHaveBeenCalledTimes(1);
  });

  it('keeps the virtual display when removal is cancelled', async () => {
    const view = await mount({ own: 2, currentId: 2 });
    await fireEvent.press(view.getByTestId('display-vd-remove'));
    await fireEvent.press(view.getByText('取消'));
    expect(
      view.core.calls.some(
        call => call.method === 'sessions.releaseVirtualDisplay',
      ),
    ).toBe(false);
    expect(useVdisplayStore.getState().owned[liveSession.id]).toBe(2);
  });

  it('keeps a pending request when the sheet closes', async () => {
    const view = await mount();
    await fireEvent.press(view.getByTestId('display-vd-create'));
    await act(async () => {
      await view.unmount();
    });
    expect(useVdisplayStore.getState().pending[liveSession.id]).toEqual({
      requestId: 1,
      deadlineMs: expect.any(Number),
    });
  });

  it('clears ownership after the owned display disappears', async () => {
    const view = await mount({ own: 7 });
    await act(async () => {
      view.core.emit({
        type: 'session.displays',
        sessionId: liveSession.id,
        displays: [displays[0]],
        currentId: 1,
      });
    });
    expect(useVdisplayStore.getState().owned[liveSession.id]).toBeUndefined();
    expect(view.getByTestId('display-vd-create')).toBeOnTheScreen();
  });
});
