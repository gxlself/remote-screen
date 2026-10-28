import { act, cleanup, fireEvent } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { deriveSessionView } from '../../features/session/view';
import { hudModel } from '../../features/session/hud';
import type { SessionRecord } from '../../features/session/store';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import {
  resetKeyboardInset,
  setKeyboardInset,
} from '../../native/keyboard/keyboardInset';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import { mockWindowDimensions } from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import { ActionFeedback } from './ActionFeedback';
import { SessionToolbar } from './SessionToolbar';
import type { SessionCtx } from './slots';

jest.mock('./IpadShortcutKeys', () => ({
  IpadShortcutKeys: () => {
    const { createElement } =
      jest.requireActual<typeof import('react')>('react');
    const { Text: ActualText } =
      jest.requireActual<typeof import('react-native')>('react-native');
    return createElement(ActualText, null, 'ipad-keys');
  },
}));

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: {
      type: 'session.stats',
      sessionId: liveSession.id,
      rttMs: 6,
      fps: 60,
      bitrateKbps: 18400,
      decodeMs: 2.1,
      lossPct: 0,
      width: 2560,
      height: 1600,
      codec: 'hevc',
      path: 'tailnetDirect',
      transport: 'quic',
    },
    displays: [
      {
        id: 1,
        name: 'Studio Display',
        widthPx: 2560,
        heightPx: 1600,
        primary: true,
      },
      {
        id: 2,
        name: '竖屏显示器',
        widthPx: 2160,
        heightPx: 3840,
        primary: false,
      },
    ],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
    ...overrides,
  };
}

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  const base = record(overrides.rec);
  const os = overrides.os ?? 'macos';
  const rec = overrides.rec ?? base;
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os,
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
    rec,
    view: deriveSessionView(rec, { os, quality: 'auto', myDeviceId: null }),
    hud: hudModel(rec),
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

async function mount(ctx: SessionCtx, core: FakeCore = createFakeCore()) {
  const view = await renderWithProviders(<SessionToolbar ctx={ctx} />, {
    core,
  });
  return { ...view, core };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('renders the macOS controls in canvas order', async () => {
  const view = await mount(context());
  const labels = [
    '收起会话，连接保持',
    '打开键盘',
    '操作方式',
    '切换显示器，当前 1 / 2',
    '画质：自动',
    '剪贴板',
    '更多',
    '断开',
  ];
  for (const label of labels) {
    expect(view.getByLabelText(label)).toBeOnTheScreen();
  }
  expect(view.queryByLabelText('发送 Ctrl Alt Delete')).toBeNull();
  expect(view.getByLabelText('会话工具栏')).toHaveProp(
    'accessibilityRole',
    'toolbar',
  );
});

it('draws the centred 44-high toolbar capsule', async () => {
  const view = await mount(context());
  expect(view.getByLabelText('会话工具栏')).toHaveStyle({
    position: 'absolute',
    bottom: 12,
    alignItems: 'center',
  });
  const bar = view.getByTestId('toolbar-disconnect').parent;
  expect(StyleSheet.flatten(bar?.props.style)).toMatchObject({
    flexDirection: 'row',
    flexWrap: 'nowrap',
    gap: 4,
    padding: 4,
    borderRadius: 28,
    borderWidth: 1,
    borderColor: 'rgba(255, 255, 255, 0.08)',
    backgroundColor: 'rgba(13, 17, 20, 0.86)',
  });
});

it('sits above the keyboard bar only while the keyboard is open', async () => {
  try {
    await act(() => setKeyboardInset(300));
    const closed = await mount(context());
    expect(closed.getByLabelText('会话工具栏')).toHaveStyle({ bottom: 12 });
    await cleanup();

    const open = await mount(context({ keyboardOpen: true }));
    expect(open.getByLabelText('会话工具栏')).toHaveStyle({ bottom: 312 });
    expect(open.getByLabelText('会话工具栏')).toContainElement(
      open.getByLabelText('收起键盘'),
    );
    await act(() => resetKeyboardInset());
    expect(open.getByLabelText('会话工具栏')).toHaveStyle({ bottom: 12 });
  } finally {
    resetKeyboardInset();
  }
});

it('wires every toolbar action', async () => {
  const ctx = context();
  const view = await mount(ctx);
  await fireEvent.press(view.getByLabelText('收起会话，连接保持'));
  await fireEvent.press(view.getByLabelText('打开键盘'));
  await fireEvent.press(view.getByLabelText('切换显示器，当前 1 / 2'));
  await fireEvent.press(view.getByLabelText('画质：自动'));
  await fireEvent.press(view.getByLabelText('剪贴板'));
  await fireEvent.press(view.getByLabelText('更多'));
  await fireEvent.press(view.getByLabelText('断开'));
  await fireEvent.press(view.getByLabelText('触屏'));
  expect(ctx.collapse).toHaveBeenCalledTimes(1);
  expect(ctx.toggleKeyboard).toHaveBeenCalledTimes(1);
  expect(ctx.openSheet).toHaveBeenNthCalledWith(1, 'displays');
  expect(ctx.openSheet).toHaveBeenNthCalledWith(2, 'quality');
  expect(ctx.openSheet).toHaveBeenNthCalledWith(3, 'clipboard');
  expect(ctx.openSheet).toHaveBeenNthCalledWith(4, 'more');
  expect(ctx.disconnect).toHaveBeenCalledTimes(1);
  expect(ctx.setMode).toHaveBeenCalledWith('touch');
});

it('uses the alternate keyboard label and icon while open', async () => {
  const view = await mount(context({ keyboardOpen: true }));
  expect(view.getByLabelText('收起键盘')).toBeOnTheScreen();
  expect(view.queryByLabelText('打开键盘')).toBeNull();
});

it('sends Ctrl+Alt+Del on Windows and keeps the mode switch', async () => {
  const multiDisplay = record();
  const ctx = context({ os: 'windows', rec: multiDisplay });
  const core = createFakeCore();
  const view = await mount(ctx, core);
  // R17 / R04：Windows 主机也能切操作方式，多块屏时也能切显示器。
  expect(view.getByLabelText('操作方式')).toBeOnTheScreen();
  expect(view.getByLabelText(/切换显示器/)).toBeOnTheScreen();
  expect(view.getByTestId('toolbar-ctrl-alt-del')).toHaveStyle({
    height: 44,
    paddingHorizontal: 12,
  });
  await fireEvent.press(view.getByLabelText('发送 Ctrl Alt Delete'));
  expect(core.calls).toContainEqual({
    method: 'sessions.action',
    args: [liveSession.id, 'ctrlAltDel'],
  });
  // R25：不再先报「已发送」，等结果（见下一条）。
  expect(ctx.showToast).not.toHaveBeenCalled();
});

it('confirms Ctrl+Alt+Del only after no failure arrives', async () => {
  jest.useFakeTimers();
  try {
    const ctx = context({ os: 'windows' });
    const core = createFakeCore();
    const view = await renderWithProviders(
      <>
        <SessionToolbar ctx={ctx} />
        <ActionFeedback ctx={ctx} />
      </>,
      { core },
    );
    await fireEvent.press(view.getByLabelText('发送 Ctrl Alt Delete'));
    await act(async () => {
      await jest.advanceTimersByTimeAsync(1000);
    });
    expect(ctx.showToast).not.toHaveBeenCalled();
    await act(async () => {
      await jest.advanceTimersByTimeAsync(600);
    });
    expect(ctx.showToast).toHaveBeenCalledWith('已发送 Ctrl+Alt+Del');
  } finally {
    jest.useRealTimers();
  }
});

it('reports a Ctrl+Alt+Del that could not be sent at all', async () => {
  const ctx = context({ os: 'windows' });
  const core = createFakeCore();
  core.failNext('sessions.action', new Error('closed'));
  const view = await mount(ctx, core);
  await fireEvent.press(view.getByLabelText('发送 Ctrl Alt Delete'));
  expect(ctx.showToast).toHaveBeenCalledWith(sessionToolbar.ctrlAltDelFailed, {
    tone: 'bad',
  });
});

it('shows why Ctrl+Alt+Del failed when the host returns code 101', async () => {
  const ctx = context({ os: 'windows' });
  const core = createFakeCore();
  const view = await renderWithProviders(
    <>
      <SessionToolbar ctx={ctx} />
      <ActionFeedback ctx={ctx} />
    </>,
    { core },
  );
  await fireEvent.press(view.getByLabelText('发送 Ctrl Alt Delete'));
  await act(() =>
    core.emit({
      type: 'session.error',
      sessionId: liveSession.id,
      hostId: liveSession.hostId,
      code: 101,
    }),
  );
  expect(ctx.showToast).toHaveBeenCalledWith(sessionToolbar.ctrlAltDelDenied, {
    tone: 'bad',
  });
});

it('shows an icon-only display entry when only the virtual display option exists', async () => {
  const base = context();
  const ctx = context({
    view: {
      ...base.view,
      toolbar: {
        ...base.view.toolbar,
        displayLabel: null,
        displayA11y: '显示器和虚拟屏',
      },
    },
  });
  const view = await mount(ctx);
  await fireEvent.press(view.getByLabelText('显示器和虚拟屏'));
  expect(ctx.openSheet).toHaveBeenCalledWith('displays');
  expect(view.getByTestId('toolbar-displays')).toBeOnTheScreen();
});

it('disables the keyboard for a viewer', async () => {
  const rec = record({ role: 'viewer' });
  const view = await mount(context({ rec }));
  const keyboard = view.getByLabelText('键盘，观看时不可用');
  expect(keyboard.props.accessibilityState.disabled).toBe(true);
  expect(keyboard).toBeDisabled();
});

it.each([
  [640, false],
  [844, true],
])('at width %s shows clipboard: %s', async (width, shown) => {
  const restore = mockWindowDimensions({ width, height: 390 });
  const view = await mount(context());
  if (shown) {
    expect(view.getByLabelText('剪贴板')).toBeOnTheScreen();
  } else {
    expect(view.queryByLabelText('剪贴板')).toBeNull();
  }
  restore();
});

it('mounts the iPad shortcut slot only on iPad', async () => {
  const first = await mount(context({ isPad: false }));
  expect(first.queryByText('ipad-keys')).toBeNull();
  const second = await mount(context({ isPad: true }));
  expect(second.getByText('ipad-keys')).toBeOnTheScreen();
});

it('reads the quality path, latency and frame rate as the quality hint', async () => {
  const ctx = context();
  const view = await mount(ctx);
  expect(view.getByTestId('toolbar-quality')).toHaveProp(
    'accessibilityHint',
    hudModel(ctx.rec).a11y,
  );
  expect(view.getByTestId('toolbar-displays')).not.toHaveProp(
    'accessibilityHint',
  );
});
