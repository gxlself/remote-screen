import { fireEvent, screen } from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import { mockWindowDimensions } from '../../test/dimensions';
import type { SessionRecord } from '../../features/session/store';
import type { Computer } from '../../native/rscore/types';
import { AUDIO_OPUS_BIT } from '../../features/session/remoteAudio';
import { clearVideo, setViewport } from '../../features/session/videoStore';
import MoreSheet from './MoreSheet';
import type { SessionCtx } from './slots';

function record(): SessionRecord {
  return {
    id: 's1',
    hostId: 'host-mac',
    hostName: 'Studio Mac mini',
    phase: 'streaming',
    role: 'controller',
    path: 'tailnetDirect',
    transport: 'quic',
    startedAt: 1,
    displayId: 1,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays: [],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
  };
}

function ctxFor(
  os: 'macos' | 'windows' = 'macos',
  host: Computer | undefined = undefined,
) {
  const rec = record();
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os,
    host,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
    rec,
    view: {
      overlay: 'none' as const,
      end: { kind: 'stay' as const },
      role: 'controller' as const,
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
        showCtrlAltDel: os === 'windows',
        showModeSwitch: os === 'macos',
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect' as const,
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '正在测量',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad' as const,
    setMode: jest.fn(),
    quality: 'auto' as const,
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
  } satisfies SessionCtx;
}

async function renderSheet(
  os: 'macos' | 'windows' = 'macos',
  settings: Record<string, unknown> = {},
  host: Computer | undefined = undefined,
) {
  const ctx = ctxFor(os, host);
  const onClose = jest.fn();
  const view = await renderWithProviders(
    <MoreSheet ctx={ctx} onClose={onClose} />,
    { settings },
  );
  return { ...view, ctx, onClose };
}

async function renderCtx(ctx: SessionCtx) {
  const onClose = jest.fn();
  const view = await renderWithProviders(
    <MoreSheet ctx={ctx} onClose={onClose} />,
  );
  return { ...view, ctx, onClose };
}

it('shows the Mac actions and hides entries with no interface', async () => {
  await renderSheet();
  expect(screen.getByText('锁定屏幕')).toBeOnTheScreen();
  expect(screen.getByText('手势说明')).toBeOnTheScreen();
  expect(screen.getByText('结束后锁屏')).toBeOnTheScreen();
  expect(screen.getByText('自然滚动')).toBeOnTheScreen();
  // R35：统计浮层只放在「连接与画质」里。
  expect(screen.queryByText('统计浮层')).toBeNull();
  expect(screen.getByText('断开')).toBeOnTheScreen();
  for (const text of [
    '发送文件',
    '截图',
    '强制退出应用',
    '工具栏位置',
    '发送 Ctrl Alt Delete',
  ]) {
    expect(screen.queryByText(text)).toBeNull();
  }
});

it('shows a usable remote sound switch when the host advertises audio', async () => {
  const host = { features: AUDIO_OPUS_BIT } as Computer;
  const view = await renderSheet('macos', {}, host);
  expect(screen.getByText('远端声音')).toBeOnTheScreen();
  expect(screen.getByText('只看画面')).toBeOnTheScreen();
  const sound = screen.getByLabelText('远端声音');
  expect(sound).not.toBeDisabled();
  await fireEvent.press(sound);
  expect(view.settingsStore.get('session.sound')).toBe(true);
  expect(screen.getByText('正在播放这台电脑的声音')).toBeOnTheScreen();
});

it('disables remote sound until the host is 0.4.0', async () => {
  const host = { features: 0 } as Computer;
  await renderSheet('macos', {}, host);
  const sound = screen.getByLabelText('远端声音');
  expect(sound).toBeDisabled();
  expect(screen.getByText('需要 Host 0.4.0')).toBeOnTheScreen();
});

it('shows Ctrl+Alt+Del only on Windows', async () => {
  await renderSheet('windows');
  expect(screen.getByText('Ctrl+Alt+Del')).toBeOnTheScreen();
  expect(screen.getByLabelText('发送 Ctrl Alt Delete')).toBeOnTheScreen();
});

it('locks the remote after confirming and closes', async () => {
  const view = await renderSheet();
  await fireEvent.press(screen.getByText('锁定屏幕'));
  // R36：锁屏先确认。
  expect(
    view.core.calls.some(
      call => call.method === 'sessions.action' && call.args[1] === 'lock',
    ),
  ).toBe(false);
  expect(screen.getByText('锁定 Studio Mac mini 的屏幕？')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('more-lock-confirm'));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.action',
    args: ['s1', 'lock', undefined],
  });
  expect(view.ctx.showToast).toHaveBeenCalledWith(
    '已锁定 Studio Mac mini 的屏幕',
  );
  expect(view.onClose).toHaveBeenCalledTimes(1);
});

it('persists lockOnEnd and informs the host', async () => {
  const view = await renderSheet();
  await fireEvent.press(screen.getByLabelText('结束后锁屏'));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.action',
    args: ['s1', 'lockOnEnd', true],
  });
  expect(view.settingsStore.get('hosts.prefs')['host-mac']?.lockOnEnd).toBe(
    true,
  );
});

it('closes then opens the gesture coach', async () => {
  const view = await renderSheet();
  await fireEvent.press(screen.getByText('手势说明'));
  expect(view.onClose).toHaveBeenCalledTimes(1);
  expect(view.ctx.openCoach).toHaveBeenCalledTimes(1);
});

it('toggles natural scroll with subtitle copy', async () => {
  const view = await renderSheet();
  expect(screen.getByText('跟随 Mac 设置')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('自然滚动'));
  expect(view.settingsStore.get('session.naturalScroll')).toBe(false);
  expect(screen.getByText('只改手机这边，Mac 不受影响')).toBeOnTheScreen();
});

it('words natural scroll and lock for a Windows host', async () => {
  await renderSheet('windows');
  expect(screen.getByText('跟随电脑设置')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('锁定屏幕'));
  expect(
    screen.getByText('电脑会回到锁屏界面，要输密码才能继续用。'),
  ).toBeOnTheScreen();
  // 主机名里带 Mac 字样，只检查文案本身不提 Mac。
  expect(screen.queryByText(/Mac 设置|Mac 不受|Mac 会/)).toBeNull();
});

it('reports a lock that could not be sent', async () => {
  const view = await renderSheet();
  view.core.failNext('sessions.action', new Error('closed'));
  await fireEvent.press(screen.getByText('锁定屏幕'));
  await fireEvent.press(screen.getByTestId('more-lock-confirm'));
  expect(view.ctx.showToast).toHaveBeenCalledWith('锁屏没有发出去，请重试', {
    tone: 'bad',
  });
});

it('keeps zoom reset visible but disabled until the picture is zoomed', async () => {
  const view = await renderSheet();
  const row = screen.getByTestId('more-zoom-reset');
  expect(row).toBeDisabled();
  expect(screen.getByText('现在没有放大')).toBeOnTheScreen();
  await view.unmount();

  setViewport('s1', { zoom: 2, x: 0.5, y: 0.5 });
  try {
    const zoomReset = jest.fn();
    await renderCtx({
      ...ctxFor(),
      videoRef: { current: { zoomReset } as never },
    });
    expect(screen.getByTestId('more-zoom-reset')).not.toBeDisabled();
    await fireEvent.press(screen.getByTestId('more-zoom-reset'));
    expect(zoomReset).toHaveBeenCalledTimes(1);
  } finally {
    clearVideo('s1');
  }
});

it('offers the display entry when the toolbar has one', async () => {
  const base = ctxFor();
  const view = await renderCtx({
    ...base,
    view: {
      ...base.view,
      toolbar: { ...base.view.toolbar, displayA11y: '显示器和虚拟屏' },
    },
  });
  await fireEvent.press(screen.getByTestId('more-displays'));
  expect(view.ctx.openSheet).toHaveBeenCalledWith('displays');
});

it('sends Ctrl+Alt+Del from More and waits for the result', async () => {
  const view = await renderSheet('windows');
  await fireEvent.press(screen.getByText('Ctrl+Alt+Del'));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.action',
    args: ['s1', 'ctrlAltDel'],
  });
  expect(view.ctx.showToast).not.toHaveBeenCalled();
  expect(view.onClose).toHaveBeenCalledTimes(1);
});

it('disconnects', async () => {
  const view = await renderSheet();
  await fireEvent.press(screen.getByText('断开'));
  expect(view.ctx.disconnect).toHaveBeenCalledTimes(1);
});

it('adds the clipboard row below 700 wide and opens that sheet', async () => {
  const restore = mockWindowDimensions({ width: 640, height: 360 });
  try {
    const view = await renderSheet();
    await fireEvent.press(screen.getByText('剪贴板'));
    expect(view.ctx.openSheet).toHaveBeenCalledWith('clipboard');
  } finally {
    restore();
  }
});
