import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  screen,
} from '@testing-library/react-native';
import type { ReactNode } from 'react';
import { Alert, StyleSheet, Text } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import {
  resetActionFeedback,
  takeActionFailure,
} from '../../features/session/actionFeedback';
import { MemoryKv } from '../../features/storage/kv';
import {
  SettingsProvider,
  createSettingsStore,
} from '../../features/settings/store';
import type { SettingsStore } from '../../features/settings/store';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import { CoreProvider } from '../rscore/CoreContext';
import { createFakeCore } from '../rscore/fakeCore';
import type { FakeCore } from '../rscore/fakeCore';
import { PlatformProvider } from '../platform/PlatformContext';
import { createFakePlatform } from '../platform/fakePlatform';
import type { FakePlatform } from '../platform/fakePlatform';
import { SessionKeyboard, keyBarPadding } from './SessionKeyboard';
import { useKeyboardInset } from './keyboardInset';

const mockLastProps: Record<string, unknown> = {};

jest.mock('../specs/RsKeyCaptureNativeComponent', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View } =
    jest.requireActual<typeof import('react-native')>('react-native');
  return {
    __esModule: true,
    default: (props: Record<string, unknown>) => {
      Object.assign(mockLastProps, props);
      return React.createElement(View, { ...props, testID: 'keycapture' });
    },
  };
});

type Options = {
  open?: boolean;
  sessionId?: string | null;
  os?: 'macos' | 'windows';
  leading?: ReactNode;
  settings?: SettingsStore;
  onBeforePaste?: () => Promise<void>;
};

async function mount(options: Options = {}) {
  const core: FakeCore = createFakeCore();
  const platform: FakePlatform = createFakePlatform();
  const settings =
    options.settings ?? createSettingsStore(new MemoryKv(), core);
  const onClose = jest.fn();
  const view = await render(
    <CoreProvider core={core}>
      <PlatformProvider platform={platform}>
        <SettingsProvider store={settings}>
          <SessionKeyboard
            open={options.open ?? true}
            sessionId={
              options.sessionId === undefined ? 's-keyboard' : options.sessionId
            }
            os={options.os ?? 'macos'}
            leading={options.leading}
            onBeforePaste={options.onBeforePaste}
            onClose={onClose}
          />
        </SettingsProvider>
      </PlatformProvider>
    </CoreProvider>,
  );
  return { view, core, platform, settings, onClose };
}

function capture() {
  return screen.getByTestId('keycapture', { includeHiddenElements: true });
}

/** Pressability 只在记下了响应者时才处理松手，模拟一个可量尺寸的响应者。 */
const responder = { measure: () => undefined };

/** Pressable 至少按住 130 ms 才报松手，等它过去。 */
async function settlePressOut() {
  await act(async () => {
    await new Promise(resolve => setTimeout(resolve, 200));
  });
}

function keyCalls(core: FakeCore) {
  return core.calls.filter(call => call.method === 'sessions.key');
}

function modifierOrder() {
  return screen
    .getAllByTestId(/^keybar-mod-/)
    .map(modifier => modifier.props.testID);
}

beforeEach(() => {
  for (const key of Object.keys(mockLastProps)) {
    delete mockLastProps[key];
  }
});

afterEach(() => {
  cleanup();
});

it('keeps KeyCapture mounted and hides the bar while closed', async () => {
  const { view } = await mount({ open: false });
  expect(capture()).toHaveProp('active', false);
  expect(screen.queryByTestId('keybar')).toBeNull();

  await view.rerender(
    <CoreProvider core={createFakeCore()}>
      <PlatformProvider platform={createFakePlatform()}>
        <SettingsProvider store={createSettingsStore(new MemoryKv())}>
          <SessionKeyboard
            open
            sessionId="s-keyboard"
            os="macos"
            onClose={() => {}}
          />
        </SettingsProvider>
      </PlatformProvider>
    </CoreProvider>,
  );
  expect(screen.getByTestId('keybar')).toBeOnTheScreen();
});

it('switches the native capture between text and key input', async () => {
  await mount();
  expect(capture()).toHaveProp('inputMode', 'text');
  expect(
    screen.getByText('文本模式：在手机上用输入法打完整句，再发送到电脑'),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('按键输入'));
  expect(capture()).toHaveProp('inputMode', 'keys');
  expect(
    screen.getByText('按键模式：每个键原样发送，适合快捷键和 vim'),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('文本输入'));
  expect(capture()).toHaveProp('inputMode', 'text');
});

it('cycles a modifier and consumes only the one-shot state', async () => {
  await mount();
  const command = screen.getByLabelText('Command，未按下');
  await fireEvent.press(command);
  expect(screen.getByLabelText('Command，单次')).toBeOnTheScreen();
  expect(capture()).toHaveProp('stickyMods', 8);

  await fireEvent(capture(), 'stickyConsumed', { nativeEvent: { mods: 8 } });
  expect(screen.getByLabelText('Command，未按下')).toBeOnTheScreen();
  expect(capture()).toHaveProp('stickyMods', 0);

  await fireEvent.press(screen.getByLabelText('Command，未按下'));
  await fireEvent.press(screen.getByLabelText('Command，单次'));
  expect(screen.getByLabelText('Command，已锁定')).toBeOnTheScreen();
  await fireEvent(capture(), 'stickyConsumed', { nativeEvent: { mods: 8 } });
  expect(screen.getByLabelText('Command，已锁定')).toBeOnTheScreen();
  expect(capture()).toHaveProp('stickyMods', 8);
});

it('sends esc with the sticky modifier and releases it afterwards', async () => {
  const { core } = await mount();
  await fireEvent.press(screen.getByLabelText('Command，未按下'));
  await fireEvent.press(screen.getByTestId('keybar-esc'));

  expect(keyCalls(core)).toEqual([
    { method: 'sessions.key', args: ['s-keyboard', 0xe3, true, 8] },
    { method: 'sessions.key', args: ['s-keyboard', 0x29, true, 8] },
    { method: 'sessions.key', args: ['s-keyboard', 0x29, false, 8] },
    { method: 'sessions.key', args: ['s-keyboard', 0xe3, false, 0] },
  ]);
  expect(screen.getByLabelText('Command，未按下')).toBeOnTheScreen();
});

it('adds sticky modifiers to shortcut key sequences', async () => {
  const { core } = await mount();
  await fireEvent.press(screen.getByLabelText('Shift，未按下'));
  await fireEvent.press(screen.getByTestId('keybar-shortcut-cmdC'));

  expect(keyCalls(core)).toEqual([
    { method: 'sessions.key', args: ['s-keyboard', 0xe1, true, 0x02] },
    { method: 'sessions.key', args: ['s-keyboard', 0xe3, true, 0x0a] },
    { method: 'sessions.key', args: ['s-keyboard', 0x06, true, 0x0a] },
    { method: 'sessions.key', args: ['s-keyboard', 0x06, false, 0x0a] },
    { method: 'sessions.key', args: ['s-keyboard', 0xe3, false, 0x02] },
    { method: 'sessions.key', args: ['s-keyboard', 0xe1, false, 0x00] },
  ]);
  expect(screen.getByLabelText('Shift，未按下')).toBeOnTheScreen();
});

it('sends system combos through sessions.combo', async () => {
  const { core } = await mount();
  await fireEvent.press(screen.getByLabelText('切换应用 Command Tab'));
  expect(core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-keyboard', 'cmdTab'],
  });
});

it('uses Ctrl+Alt+Del as a session action and pastes clipboard text on Windows', async () => {
  const { core, platform } = await mount({ os: 'windows' });
  await fireEvent.press(screen.getByLabelText('安全选项 Ctrl Alt Delete'));
  expect(core.calls).toContainEqual({
    method: 'sessions.action',
    args: ['s-keyboard', 'ctrlAltDel'],
  });

  platform.clipboardText = 'hi';
  await fireEvent.press(
    screen.getByLabelText('粘贴为键入：把手机剪贴板逐字敲进去'),
  );
  await act(async () => {});
  expect(core.calls).toContainEqual({
    method: 'sessions.text',
    args: ['s-keyboard', 'hi'],
  });
});

it('notes Ctrl+Alt+Del for failure feedback but not modifier locks', async () => {
  resetActionFeedback();
  await mount({ os: 'windows' });
  await fireEvent.press(screen.getByLabelText('安全选项 Ctrl Alt Delete'));
  expect(
    takeActionFailure({
      type: 'session.error',
      sessionId: 's-keyboard',
      hostId: 'h1',
      code: 101,
    }),
  ).toBe(sessionToolbar.ctrlAltDelDenied);

  resetActionFeedback();
  await fireEvent.press(screen.getByLabelText('Win，未按下'));
  await fireEvent.press(screen.getByLabelText('Win，单次'));
  expect(screen.getByLabelText('Win，已锁定')).toBeOnTheScreen();
  expect(
    takeActionFailure({
      type: 'session.error',
      sessionId: 's-keyboard',
      hostId: 'h1',
      code: 101,
    }),
  ).toBeNull();
});

it('shows the Command-as-Ctrl switch only for Windows and updates KeyCapture', async () => {
  const { settings } = await mount({ os: 'windows' });
  const toggle = screen.getByRole('switch');
  expect(toggle).toHaveProp('accessibilityState', { checked: true });
  expect(capture()).toHaveProp('os', 'windows');
  expect(screen.getByText('Ctrl')).toBeOnTheScreen();
  expect(screen.getByText('Alt')).toBeOnTheScreen();
  expect(screen.getByText('Win')).toBeOnTheScreen();
  expect(screen.getByText('Shift')).toBeOnTheScreen();
  expect(modifierOrder()).toEqual([
    'keybar-mod-ctl',
    'keybar-mod-opt',
    'keybar-mod-cmd',
    'keybar-mod-shf',
  ]);

  await fireEvent.press(toggle);
  expect(settings.get('keyboard.cmdAsCtrl')).toBe(false);
  expect(capture()).toHaveProp('os', 'macos');
  expect(screen.getByText('Ctrl')).toBeOnTheScreen();
  expect(screen.getByText('Win')).toBeOnTheScreen();
  expect(modifierOrder()).toEqual([
    'keybar-mod-ctl',
    'keybar-mod-opt',
    'keybar-mod-cmd',
    'keybar-mod-shf',
  ]);
});

it('renders the hint as a status pill', async () => {
  await mount({ os: 'windows' });
  const hint = screen.getByTestId('keybar-hint-pill');
  expect(hint).toHaveProp('accessibilityRole', 'status');
  expect(hint).toHaveProp('accessibilityLabel', '键盘提示');
  expect(StyleSheet.flatten(hint.props.style)).toMatchObject({
    maxWidth: 340,
    flexDirection: 'row',
    alignItems: 'center',
    minHeight: 30,
    borderRadius: 15,
    backgroundColor: 'rgba(10,13,16,0.8)',
  });
  expect(
    screen.getByText('文本模式：用手机输入法打好整句，再发到电脑'),
  ).toHaveStyle({ color: '#D5DCE0', fontSize: 12 });
});

it('hides the Command-as-Ctrl switch on macOS', async () => {
  await mount({ os: 'macos' });
  expect(screen.queryByRole('switch')).toBeNull();
});

it('renders leading content on the hint row', async () => {
  await mount({
    leading: <Text testID="lead">跟随输入位置 2×</Text>,
  });
  const lead = screen.getByTestId('lead');
  const hint = screen.getByTestId('keybar-hint');
  expect(lead).toBeOnTheScreen();
  expect(hint).toContainElement(lead);
  expect(hint).toContainElement(screen.getByTestId('keybar-hint-pill'));
});

it('shows a toast when the clipboard has no text', async () => {
  jest.useFakeTimers();
  try {
    await mount({ os: 'windows' });
    await fireEvent.press(
      screen.getByLabelText('粘贴为键入：把手机剪贴板逐字敲进去'),
    );
    await act(async () => {});
    expect(screen.getByText('剪贴板里没有文字')).toBeOnTheScreen();
    await act(() => jest.advanceTimersByTime(2000));
    expect(screen.queryByText('剪贴板里没有文字')).toBeNull();
  } finally {
    jest.useRealTimers();
  }
});

it('confirms large clipboard pastes before sending', async () => {
  const alert = jest.spyOn(Alert, 'alert').mockImplementation(() => {});
  try {
    const { core, platform } = await mount({ os: 'windows' });
    platform.clipboardText = 'a'.repeat(2001);
    await fireEvent.press(
      screen.getByLabelText('粘贴为键入：把手机剪贴板逐字敲进去'),
    );
    await act(async () => {});
    expect(alert).toHaveBeenCalledWith(
      '粘贴为键入',
      '要把 2,001 个字逐字敲到电脑上吗？',
      expect.any(Array),
    );
    expect(core.calls).not.toContainEqual(
      expect.objectContaining({ method: 'sessions.text' }),
    );
    const buttons = alert.mock.calls[0][2] as Array<{
      text: string;
      onPress?: () => void;
    }>;
    await act(async () => buttons[1].onPress?.());
    expect(core.calls).toContainEqual({
      method: 'sessions.text',
      args: ['s-keyboard', 'a'.repeat(2001)],
    });
  } finally {
    alert.mockRestore();
  }
});

it('opens the function-key strip and sends F5 before returning', async () => {
  const { core } = await mount();
  await fireEvent.press(screen.getByLabelText('功能键 F1 到 F12'));
  expect(screen.getByTestId('keybar-functions')).toBeOnTheScreen();
  expect(screen.getByTestId('keybar-f12')).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('功能键 F5'));
  expect(keyCalls(core)).toContainEqual({
    method: 'sessions.key',
    args: ['s-keyboard', 0x3e, true, 0],
  });

  await fireEvent.press(screen.getByLabelText('返回'));
  expect(screen.getByTestId('keybar-main')).toBeOnTheScreen();
});

it('sends one arrow for a tap by side and repeats while dragging', async () => {
  const { core } = await mount();
  const arrows = screen.getByTestId('keybar-arrows');
  // R15：轻点也要发键，按点在键的哪一边决定方向。
  await fireEvent(arrows, 'responderGrant', {
    persist: () => {},
    currentTarget: responder,
    nativeEvent: { pageX: 0, pageY: 0, locationX: 40, locationY: 22 },
  });
  await fireEvent(arrows, 'responderRelease', {
    persist: () => {},
    nativeEvent: { pageX: 0, pageY: 0, locationX: 40, locationY: 22 },
  });
  await settlePressOut();
  expect(
    keyCalls(core)
      .filter(call => call.args[2] === true)
      .map(call => call.args[1]),
  ).toEqual([0x4f]);

  await fireEvent(arrows, 'responderGrant', {
    persist: () => {},
    currentTarget: responder,
    nativeEvent: { pageX: 0, pageY: 0, locationX: 22, locationY: 2 },
  });
  await fireEvent(arrows, 'responderRelease', {
    persist: () => {},
    nativeEvent: { pageX: 0, pageY: 0, locationX: 22, locationY: 2 },
  });
  await settlePressOut();
  expect(
    keyCalls(core)
      .filter(call => call.args[2] === true)
      .map(call => call.args[1]),
  ).toEqual([0x4f, 0x52]);

  await fireEvent(arrows, 'responderGrant', {
    persist: () => {},
    currentTarget: responder,
    nativeEvent: { pageX: 0, pageY: 0, locationX: 22, locationY: 22 },
  });
  await fireEvent(arrows, 'responderMove', {
    persist: () => {},
    nativeEvent: { pageX: 50, pageY: 0 },
  });
  await fireEvent(arrows, 'responderRelease', {
    persist: () => {},
    nativeEvent: { pageX: 50, pageY: 0 },
  });
  await settlePressOut();
  const downArrows = keyCalls(core).filter(call => call.args[2] === true);
  // 拖动发两次右键，松手时不再额外补一次。
  expect(downArrows.map(call => call.args[1])).toEqual([
    0x4f, 0x52, 0x4f, 0x4f,
  ]);
});

it('locks horizontal scrolling while the arrow key is held', async () => {
  await mount();
  const arrows = screen.getByTestId('keybar-arrows');
  await fireEvent(arrows, 'responderGrant', {
    persist: () => {},
    currentTarget: responder,
    nativeEvent: { pageX: 0, pageY: 0, locationX: 22, locationY: 22 },
  });
  expect(screen.getByTestId('keybar-main').props.scrollEnabled).toBe(false);
  await fireEvent(arrows, 'responderRelease', {
    persist: () => {},
    nativeEvent: { pageX: 0, pageY: 0, locationX: 22, locationY: 22 },
  });
  await settlePressOut();
  expect(screen.getByTestId('keybar-main').props.scrollEnabled).toBe(true);
});

it('offers arrow actions to screen readers', async () => {
  const { core } = await mount();
  const arrows = screen.getByTestId('keybar-arrows');
  expect(
    arrows.props.accessibilityActions.map(
      (action: { name: string }) => action.name,
    ),
  ).toEqual(['up', 'down', 'left', 'right']);
  await fireEvent(arrows, 'accessibilityAction', {
    nativeEvent: { actionName: 'left' },
  });
  expect(keyCalls(core)).toContainEqual({
    method: 'sessions.key',
    args: ['s-keyboard', 0x50, true, 0],
  });
});

it('tells the user when a key could not be sent', async () => {
  const { core } = await mount();
  core.failNext('sessions.key', new Error('closed'));
  await fireEvent.press(screen.getByTestId('keybar-esc'));
  expect(screen.getByTestId('keybar-toast')).toHaveTextContent(
    '按键没发出去，请重试',
  );
});

it('shows a pressed state on key bar buttons', async () => {
  await mount();
  const esc = screen.getByTestId('keybar-esc');
  await fireEvent(esc, 'responderGrant', {
    persist: () => {},
    currentTarget: responder,
    nativeEvent: { pageX: 0, pageY: 0, locationX: 0, locationY: 0 },
  });
  expect(screen.getByTestId('keybar-esc')).toHaveStyle({
    backgroundColor: '#39434B',
  });
});

it('adds close-window and quit shortcuts on Mac and Alt+F4 and Win on Windows', async () => {
  const mac = await mount();
  await fireEvent.press(screen.getByLabelText('关闭窗口 Command W'));
  expect(keyCalls(mac.core)).toContainEqual({
    method: 'sessions.key',
    args: ['s-keyboard', 0x1a, true, 8],
  });
  expect(screen.getByLabelText('退出应用 Command Q')).toBeOnTheScreen();
  await mac.view.unmount();

  const windows = await mount({ os: 'windows' });
  await fireEvent.press(screen.getByLabelText('关闭窗口 Alt F4'));
  expect(keyCalls(windows.core)).toContainEqual({
    method: 'sessions.key',
    args: ['s-keyboard', 0x3d, true, 4],
  });
  await fireEvent.press(screen.getByLabelText('开始菜单 Win 键'));
  expect(windows.core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-keyboard', 'winKey'],
  });
});

it('closes on active-change and the close button', async () => {
  const { view, onClose } = await mount();
  await fireEvent(capture(), 'activeChange', {
    nativeEvent: { active: false },
  });
  expect(onClose).toHaveBeenCalledTimes(1);
  await fireEvent.press(screen.getByTestId('keybar-close'));
  expect(onClose).toHaveBeenCalledTimes(2);

  await view.rerender(
    <CoreProvider core={createFakeCore()}>
      <PlatformProvider platform={createFakePlatform()}>
        <SettingsProvider store={createSettingsStore(new MemoryKv())}>
          <SessionKeyboard
            open={false}
            sessionId="s-keyboard"
            os="macos"
            onClose={onClose}
          />
        </SettingsProvider>
      </PlatformProvider>
    </CoreProvider>,
  );
  expect(capture()).toHaveProp('active', false);
});

function ancestorTestIds(element: ReturnType<typeof screen.getByTestId>) {
  const ids: string[] = [];
  for (let node = element.parent; node !== null; node = node.parent) {
    const id: unknown = node.props.testID;
    if (typeof id === 'string') {
      ids.push(id);
    }
  }
  return ids;
}

it('pins the close button outside the scrolling key row', async () => {
  const { onClose } = await mount();
  const close = screen.getByTestId('keybar-close');
  expect(ancestorTestIds(close)).toContain('keybar');
  expect(ancestorTestIds(close)).not.toContain('keybar-main');
  expect(screen.getByTestId('keybar-main')).not.toContainElement(close);
  await fireEvent.press(close);
  expect(onClose).toHaveBeenCalledTimes(1);

  await fireEvent.press(screen.getByLabelText('功能键 F1 到 F12'));
  const pinned = screen.getByTestId('keybar-close');
  expect(ancestorTestIds(pinned)).not.toContain('keybar-functions');
  await fireEvent.press(pinned);
  expect(onClose).toHaveBeenCalledTimes(2);
});

it('disables session shortcuts without a session id', async () => {
  const { core } = await mount({ sessionId: null });
  const combo = screen.getByLabelText('切换应用 Command Tab');
  expect(combo).toBeDisabled();
  await fireEvent.press(combo);
  expect(core.calls.filter(call => call.method === 'sessions.combo')).toEqual(
    [],
  );
});

it('writes the combined keyboard inset and clears it when closed', async () => {
  const inset = await renderHook(() => useKeyboardInset());
  const { view } = await mount();
  await fireEvent(capture(), 'keyboardFrame', {
    nativeEvent: { height: 300, visible: true, hardware: false },
  });
  expect(inset.result.current).toBeGreaterThan(300);

  await view.rerender(
    <CoreProvider core={createFakeCore()}>
      <PlatformProvider platform={createFakePlatform()}>
        <SettingsProvider store={createSettingsStore(new MemoryKv())}>
          <SessionKeyboard
            open={false}
            sessionId="s-keyboard"
            os="macos"
            onClose={() => {}}
          />
        </SettingsProvider>
      </PlatformProvider>
    </CoreProvider>,
  );
  expect(inset.result.current).toBe(0);
});

it('keeps the bar above the home indicator with a hardware keyboard', async () => {
  const safe = useSafeAreaInsets as jest.MockedFunction<
    typeof useSafeAreaInsets
  >;
  safe.mockReturnValue({ top: 0, right: 0, bottom: 34, left: 0 });
  try {
    await mount();
    await fireEvent(capture(), 'keyboardFrame', {
      nativeEvent: { height: 34, visible: true, hardware: true },
    });
    const anchor = screen.getByTestId('session-keyboard');
    expect(anchor).toHaveStyle({ bottom: 34 });
  } finally {
    safe.mockReturnValue({ top: 0, right: 0, bottom: 0, left: 0 });
  }
});

it('counts the home indicator in the inset when the bar sits on it', async () => {
  const safe = useSafeAreaInsets as jest.MockedFunction<
    typeof useSafeAreaInsets
  >;
  safe.mockReturnValue({ top: 0, right: 0, bottom: 34, left: 0 });
  try {
    const inset = await renderHook(() => useKeyboardInset());
    await mount();
    await fireEvent(capture(), 'keyboardFrame', {
      nativeEvent: { height: 34, visible: true, hardware: true },
    });
    expect(inset.result.current).toBe(34 + 52 + 44);

    await fireEvent(capture(), 'keyboardFrame', {
      nativeEvent: { height: 300, visible: true, hardware: false },
    });
    expect(inset.result.current).toBe(300 + 52 + 44);
  } finally {
    safe.mockReturnValue({ top: 0, right: 0, bottom: 0, left: 0 });
  }
});

it('shows composition text on the right of the hint', async () => {
  await mount();
  await fireEvent(capture(), 'composition', {
    nativeEvent: { text: 'nihao' },
  });
  expect(screen.getByText('组合中：nihao')).toBeOnTheScreen();
});

it('sends Home, End, Page Up and Page Down with the sticky modifiers', async () => {
  const { core } = await mount();
  await fireEvent.press(screen.getByLabelText('Home 键'));
  await fireEvent.press(screen.getByLabelText('End 键'));
  await fireEvent.press(screen.getByLabelText('向上翻页 Page Up'));
  await fireEvent.press(screen.getByLabelText('Shift，未按下'));
  await fireEvent.press(screen.getByLabelText('向下翻页 Page Down'));

  expect(keyCalls(core).map(call => call.args.slice(1))).toEqual([
    [0x4a, true, 0],
    [0x4a, false, 0],
    [0x4d, true, 0],
    [0x4d, false, 0],
    [0x4b, true, 0],
    [0x4b, false, 0],
    [0xe1, true, 0x02],
    [0x4e, true, 0x02],
    [0x4e, false, 0x02],
    [0xe1, false, 0],
  ]);
  expect(screen.getByText('pg dn')).toBeOnTheScreen();
});

it('pads the bar for the iPhone notch only, and for the real safe area on iPad', () => {
  expect(keyBarPadding(true, 0, false)).toBe(59);
  expect(keyBarPadding(true, 59, false)).toBe(71);
  expect(keyBarPadding(true, 0, true)).toBe(12);
  expect(keyBarPadding(true, 20, true)).toBe(32);
  expect(keyBarPadding(false, 0, false)).toBe(12);
  expect(keyBarPadding(false, 0, true)).toBe(12);
});

function deferred() {
  let resolve: () => void = () => undefined;
  let reject: (error: Error) => void = () => undefined;
  const promise = new Promise<void>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

it('pushes the clipboard before a hardware Command-V goes out', async () => {
  const push = deferred();
  const onBeforePaste = jest.fn(() => push.promise);
  const { core } = await mount({ onBeforePaste });
  expect(capture()).toHaveProp('syncPaste', true);

  // Command is held on the keyboard: it goes back down afterwards.
  await fireEvent(capture(), 'paste', { nativeEvent: { mods: 0x08 } });
  expect(onBeforePaste).toHaveBeenCalledTimes(1);
  expect(keyCalls(core)).toEqual([]);

  await act(async () => push.resolve());
  expect(keyCalls(core).map(call => call.args.slice(1))).toEqual([
    [0xe3, true, 0x08],
    [0x19, true, 0x08],
    [0x19, false, 0x08],
    [0xe3, false, 0],
    [0xe3, true, 0x08],
  ]);
});

it('still pastes when the push fails, and uses Ctrl on Windows', async () => {
  const push = deferred();
  const { core } = await mount({
    os: 'windows',
    onBeforePaste: () => push.promise,
  });
  expect(capture()).toHaveProp('syncPaste', true);

  // Right Command, mapped to right Ctrl by the native side.
  await fireEvent(capture(), 'paste', { nativeEvent: { mods: 0x10 } });
  await act(async () => push.reject(new Error('offline')));
  expect(keyCalls(core).map(call => call.args.slice(1))).toEqual([
    [0xe4, true, 0x10],
    [0x19, true, 0x10],
    [0x19, false, 0x10],
    [0xe4, false, 0],
    [0xe4, true, 0x10],
  ]);
});

it('syncs before the bar paste keys too', async () => {
  const push = deferred();
  const onBeforePaste = jest.fn(() => push.promise);
  const { core } = await mount({ onBeforePaste });

  await fireEvent.press(screen.getByLabelText('粘贴 Command V'));
  expect(onBeforePaste).toHaveBeenCalledTimes(1);
  expect(keyCalls(core)).toEqual([]);
  await act(async () => push.resolve());
  expect(keyCalls(core).map(call => call.args.slice(1))).toEqual([
    [0xe3, true, 0x08],
    [0x19, true, 0x08],
    [0x19, false, 0x08],
    [0xe3, false, 0],
  ]);

  // Other shortcuts never wait.
  await fireEvent.press(screen.getByLabelText('拷贝 Command C'));
  expect(onBeforePaste).toHaveBeenCalledTimes(1);
});

it('pastes straight away when the setting is off or Command is the Win key', async () => {
  const core = createFakeCore();
  const settings = createSettingsStore(new MemoryKv(), core);
  settings.set('keyboard.pasteSync', false);
  const onBeforePaste = jest.fn(() => Promise.resolve());
  const off = await mount({ settings, onBeforePaste });
  expect(capture()).toHaveProp('syncPaste', false);
  await fireEvent(capture(), 'paste', { nativeEvent: { mods: 0 } });
  expect(keyCalls(off.core).map(call => call.args.slice(1))).toEqual([
    [0xe3, true, 0x08],
    [0x19, true, 0x08],
    [0x19, false, 0x08],
    [0xe3, false, 0],
  ]);
  await off.view.unmount();

  const winKey = createSettingsStore(new MemoryKv(), core);
  winKey.set('keyboard.cmdAsCtrl', false);
  const win = await mount({ os: 'windows', settings: winKey, onBeforePaste });
  expect(capture()).toHaveProp('syncPaste', false);
  await win.view.unmount();

  // No push to wait for: a Host session page passes none.
  await mount();
  expect(capture()).toHaveProp('syncPaste', false);
  expect(onBeforePaste).not.toHaveBeenCalled();
});
