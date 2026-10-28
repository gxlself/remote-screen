import { fireEvent, render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { directory, liveSession } from '../../native/rscore/fixtures';
import type { SessionCtx } from './slots';
import GestureCoach from './GestureCoach';
import { mockWindowDimensions } from '../../test/dimensions';

function sessionCtx(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: directory.computers[0].hostId,
    hostName: directory.computers[0].name,
    os: 'macos',
    host: directory.computers[0],
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
      a11y: '连接质量：直连 6 毫秒，60 帧，查看详情',
    },
    isPad: false,
    reduceMotion: false,
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

async function mount(
  ctx: Partial<SessionCtx> = {},
  onClose: () => void = jest.fn(),
) {
  const resolved = sessionCtx(ctx);
  await render(<GestureCoach ctx={resolved} onClose={onClose} />);
  return { ctx: resolved, onClose };
}

it('renders trackpad content and the canvas grid', async () => {
  const view = await mount({ mode: 'trackpad' });

  expect(screen.getByText('触控板模式：整块屏幕都是触控板')).toBeOnTheScreen();
  expect(screen.getByLabelText('三指轻点：显示或收起工具栏')).toBeOnTheScreen();
  expect(
    screen.getByText('长按半秒再拖动 = 按住鼠标拖拽、选中文字'),
  ).toBeOnTheScreen();
  expect(screen.getByText('改用触屏')).toBeOnTheScreen();
  expect(screen.getByText('开始使用')).toBeOnTheScreen();

  const overlay = StyleSheet.flatten(
    screen.getByTestId('gesture-coach').props.style,
  );
  expect(overlay.backgroundColor).toBe('rgba(0,0,0,0.7)');
  expect(screen.getByTestId('gesture-coach')).toHaveProp(
    'accessibilityViewIsModal',
    true,
  );

  const content = StyleSheet.flatten(
    screen.getByTestId('gesture-coach-content').props.style,
  );
  expect(content).toMatchObject({
    position: 'absolute',
    left: 59,
    right: 59,
    top: 16,
    bottom: 14,
    gap: 12,
  });

  const cell = StyleSheet.flatten(
    screen.getByLabelText('单指滑动：移动光标，快划会加速').props.style,
  );
  expect(cell).toMatchObject({
    flex: 1,
    borderRadius: 16,
    borderWidth: 1,
    padding: 12,
    backgroundColor: '#151A1E',
  });
  expect(screen.getAllByLabelText(/：/)).toHaveLength(7);
  expect(view.onClose).not.toHaveBeenCalled();
});

it('switches mode before closing', async () => {
  const order: string[] = [];
  const ctx = sessionCtx({ mode: 'trackpad' });
  ctx.setMode = jest.fn(() => order.push('mode'));
  const onClose = jest.fn(() => order.push('close'));
  await render(<GestureCoach ctx={ctx} onClose={onClose} />);

  await fireEvent.press(screen.getByLabelText('改用触屏'));

  expect(ctx.setMode).toHaveBeenCalledWith('touch');
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(order).toEqual(['mode', 'close']);
});

it('closes from the start button without changing mode', async () => {
  const ctx = sessionCtx({ mode: 'trackpad' });
  const onClose = jest.fn();
  await render(<GestureCoach ctx={ctx} onClose={onClose} />);

  await fireEvent.press(screen.getByLabelText('开始使用'));

  expect(ctx.setMode).not.toHaveBeenCalled();
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('renders touch content with the reverse switch', async () => {
  const ctx = sessionCtx({ mode: 'touch' });
  await render(<GestureCoach ctx={ctx} onClose={jest.fn()} />);

  expect(screen.getByText('触屏模式：点哪里就点哪里')).toBeOnTheScreen();
  expect(screen.getByText('改用触控板')).toBeOnTheScreen();
  expect(screen.queryByText('改用触屏')).toBeNull();
  expect(
    screen.getByLabelText('单指拖动：光标跟着手指走，按住拖动'),
  ).toBeOnTheScreen();
});

it('falls back to ctx.closeCoach when the shell does not pass onClose', async () => {
  const ctx = sessionCtx({ mode: 'trackpad' });
  await render(<GestureCoach ctx={ctx} />);

  await fireEvent.press(screen.getByLabelText('开始使用'));

  expect(ctx.closeCoach).toHaveBeenCalledTimes(1);
});

it('scrolls in a compact layout on a short landscape phone', async () => {
  const restore = mockWindowDimensions({ width: 844, height: 390 });
  try {
    await mount();
    expect(screen.getByTestId('gesture-coach-scroll')).toBeOnTheScreen();
  } finally {
    restore();
  }
});
