import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { netTailnetOk } from '../../native/rscore/fixtures';
import { settings as copy } from '../../i18n/zh/settings';
import { mockNavigation } from '../../test/navigation';
import { MemoryKv } from '../../features/storage/kv';
import { seedLocalSync } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import SettingsScreen from '.';

jest.setTimeout(30000);

jest.mock('./NetworkSections', () => ({
  __esModule: true,
  default: () => null,
}));

async function renderScreen(
  opts: {
    core?: ReturnType<typeof createFakeCore>;
    platform?: ReturnType<typeof createFakePlatform>;
    settings?: Partial<{
      'lock.onOpen': boolean;
      'session.mode.phone': 'trackpad' | 'touch';
      'session.mode.tablet': 'trackpad' | 'touch';
      'tailnet.enabled': boolean;
    }>;
    kv?: MemoryKv;
  } = {},
) {
  const core = opts.core ?? createFakeCore();
  const platform = opts.platform ?? createFakePlatform();
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Settings'>['navigation'];
  const rendered = await renderWithProviders(
    <SettingsScreen
      navigation={navigation}
      route={
        {
          key: 'Settings-test',
          name: 'Settings',
        } as TabScreenProps<'Settings'>['route']
      }
    />,
    // The tailnet rows and header need 外网访问 (Tailscale) on.
    {
      core,
      platform,
      kv: opts.kv,
      settings: { 'tailnet.enabled': true, ...opts.settings },
    },
  );
  await act(async () => {
    await Promise.resolve();
  });
  return { ...rendered, core, platform, navigation };
}

afterEach(async () => {
  await cleanup();
});

it('shows the account header and a connected tailnet address', async () => {
  const core = createFakeCore();
  core.setFixtures({ network: netTailnetOk });
  await renderScreen({ core });
  expect(screen.getByText('设置')).toBeOnTheScreen();
  expect(screen.getByText('我的 iPhone')).toBeOnTheScreen();
  expect(screen.getByText('本机 100.64.0.21')).toBeOnTheScreen();
});

it('names no Tailscale in the header while it is switched off', async () => {
  const core = createFakeCore();
  core.setFixtures({ network: netTailnetOk });
  await renderScreen({ core, settings: { 'tailnet.enabled': false } });
  expect(screen.getByText('我的 iPhone')).toBeOnTheScreen();
  expect(screen.queryByText('本机 100.64.0.21')).toBeNull();
  expect(screen.queryByText('Tailscale 未连接')).toBeNull();
  expect(screen.queryByText('已连接')).toBeNull();
});

it('renders security values and opens each destination', async () => {
  const beforeSession = await renderScreen({
    settings: { 'lock.onOpen': false },
  });
  expect(screen.getByText('关闭')).toBeOnTheScreen();
  await beforeSession.unmount();

  const { navigation } = await renderScreen();

  expect(screen.getByText('4 台')).toBeOnTheScreen();
  expect(screen.getByText('1 把')).toBeOnTheScreen();
  expect(screen.getByText('已开启')).toBeOnTheScreen();

  await fireEvent.press(
    screen.getByLabelText(
      [
        copy.security.sshKeys.title,
        copy.security.sshKeys.subtitle,
        copy.security.sshKeys.count(1),
      ].join('，'),
    ),
  );
  expect(navigation.navigate).toHaveBeenLastCalledWith('SshKeys');
  await fireEvent.press(
    screen.getByLabelText(
      [
        copy.security.faceId.title('Face ID'),
        copy.security.faceId.subtitle,
        copy.security.faceId.valueOn,
      ].join('，'),
    ),
  );
  expect(navigation.navigate).toHaveBeenLastCalledWith('AppLock');
});

it('uses Touch ID for the security row', async () => {
  const platform = createFakePlatform();
  platform.authCapabilityResult = { biometry: 'touchId', passcode: true };
  const { navigation } = await renderScreen({ platform });

  await fireEvent.press(
    screen.getByLabelText(
      [
        copy.security.faceId.title('Touch ID'),
        copy.security.faceId.subtitle,
        copy.security.faceId.valueOn,
      ].join('，'),
    ),
  );
  expect(navigation.navigate).toHaveBeenLastCalledWith('AppLock');
});

it('changes the phone operation mode and its explanation', async () => {
  const { settingsStore } = await renderScreen();

  expect(screen.getByLabelText('触控板')).toBeSelected();
  expect(
    screen.getByText(
      '整块屏幕都是触控板，单指移动光标、轻点单击。适合手机这种小屏。',
    ),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('触屏直点'));
  expect(settingsStore.get('session.mode.phone')).toBe('touch');
  expect(
    screen.getByText('手指点到哪里就点哪里，拖动会移动窗口。适合 iPad 大屏。'),
  ).toBeOnTheScreen();
});

it('writes the tablet operation mode when the device is an iPad', async () => {
  const platform = createFakePlatform();
  platform.info.isPad = true;
  const { settingsStore } = await renderScreen({ platform });

  expect(screen.getByLabelText('触屏直点')).toBeSelected();
  await fireEvent.press(screen.getByLabelText('触控板'));
  expect(settingsStore.get('session.mode.tablet')).toBe('trackpad');
});

it('selects a default quality from the option sheet', async () => {
  const { settingsStore } = await renderScreen();

  expect(screen.getByText('自动')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('画质，自动'));
  expect(screen.getByText('默认画质')).toBeOnTheScreen();
  expect(screen.getByLabelText('清晰')).not.toBeSelected();
  await fireEvent.press(screen.getByLabelText('清晰'));
  expect(settingsStore.get('quality.default')).toBe('clear');
  expect(screen.queryByText('默认画质')).toBeNull();
  expect(screen.getByText('清晰')).toBeOnTheScreen();
});

it('steps cursor speed by 0.2 within 0.6–2.4', async () => {
  const { settingsStore } = await renderScreen();
  const faster = screen.getByLabelText('光标快一点');
  const slower = screen.getByLabelText('光标慢一点');
  const adjustable = screen.getByLabelText('光标速度');

  expect(adjustable).toHaveProp('accessibilityValue', { text: '1.4 倍' });
  await fireEvent.press(faster);
  await fireEvent.press(faster);
  expect(screen.getByText('1.8×')).toBeOnTheScreen();

  for (let index = 0; index < 5; index++) {
    await fireEvent.press(faster);
  }
  expect(settingsStore.get('session.cursorSpeed')).toBe(2.4);
  await fireEvent.press(faster);
  expect(settingsStore.get('session.cursorSpeed')).toBe(2.4);

  for (let index = 0; index < 12; index++) {
    await fireEvent.press(slower);
  }
  expect(settingsStore.get('session.cursorSpeed')).toBe(0.6);
});

it('supports adjustable accessibility increment and decrement', async () => {
  const { settingsStore } = await renderScreen();
  const adjustable = screen.getByLabelText('光标速度');

  await fireEvent(adjustable, 'accessibilityAction', {
    nativeEvent: { actionName: 'increment' },
  });
  expect(settingsStore.get('session.cursorSpeed')).toBe(1.6);
  await fireEvent(adjustable, 'accessibilityAction', {
    nativeEvent: { actionName: 'decrement' },
  });
  expect(settingsStore.get('session.cursorSpeed')).toBe(1.4);
});

it('toggles natural scrolling, haptics and remote sound', async () => {
  const { settingsStore } = await renderScreen();

  await fireEvent.press(screen.getByLabelText('自然滚动'));
  expect(settingsStore.get('session.naturalScroll')).toBe(false);
  await fireEvent.press(screen.getByLabelText('点击时轻震'));
  expect(settingsStore.get('session.haptics')).toBe(false);
  const sound = screen.getByLabelText('播放远端声音');
  expect(sound).not.toBeDisabled();
  await fireEvent.press(sound);
  expect(settingsStore.get('session.sound')).toBe(true);
});

it('syncs the clipboard before pasting by default and can turn it off', async () => {
  const { settingsStore } = await renderScreen();

  expect(settingsStore.get('keyboard.pasteSync')).toBe(true);
  expect(
    screen.getByText(
      '屏幕共享、远程桌面里粘贴时，先把这台设备的剪贴板发到电脑。屏幕共享只能带英文和符号',
    ),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('粘贴前先发剪贴板'));
  expect(settingsStore.get('keyboard.pasteSync')).toBe(false);
  // The Command-period note is for iPad keyboards only.
  expect(screen.queryByTestId('settings-command-period')).toBeNull();
});

it('explains Command-period as Esc on iPad', async () => {
  const platform = createFakePlatform();
  platform.info.isPad = true;
  await renderScreen({ platform });

  expect(
    screen.getByText('外接键盘没有 Esc 键时，按 ⌘. 发送 Esc'),
  ).toBeOnTheScreen();
});

it('toggles automatic tmux reattach', async () => {
  const { settingsStore } = await renderScreen();

  await fireEvent.press(screen.getByLabelText('自动接回 tmux'));
  expect(settingsStore.get('terminal.tmux')).toBe(false);
});

it('changes terminal font size and opens the snippets editor', async () => {
  const kv = new MemoryKv();
  seedLocalSync(kv, 'snippets', {
    version: 1,
    value: {
      v: 1,
      items: [
        { id: 'a', label: 'A', cmd: 'echo a', run: true },
        { id: 'b', label: 'B', cmd: 'echo b', run: true },
        { id: 'c', label: 'C', cmd: 'echo c', run: false },
      ],
    },
  });
  const { settingsStore } = await renderScreen({ kv });

  // S19: the row changes the size only, so it says 字号 and no font name.
  expect(screen.queryByText(/Geist Mono/)).toBeNull();
  await fireEvent.press(screen.getByLabelText('字号，13'));
  expect(screen.getByText('终端字号')).toBeOnTheScreen();
  // T26: sizes go up to 20.
  expect(screen.getByRole('radio', { name: '20' })).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('radio', { name: '15' }));
  expect(settingsStore.get('terminal.fontSize')).toBe(15);
  expect(screen.queryByText('终端字号')).toBeNull();

  expect(screen.getByText('3 条')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('命令片段，3 条'));
  expect(screen.getAllByText('命令片段').length).toBeGreaterThanOrEqual(2);
});

it('opens diagnostics, exposes dev self-test and renders the footer', async () => {
  const { navigation } = await renderScreen();

  expect(screen.queryByText('网络接入')).toBeNull();
  expect(screen.queryByText('远程唤醒')).toBeNull();
  await fireEvent.press(
    screen.getByLabelText(
      [copy.diagnostics.title, copy.diagnostics.subtitle].join('，'),
    ),
  );
  expect(navigation.navigate).toHaveBeenLastCalledWith('Diagnostics');
  expect(screen.getByLabelText('开发者自检')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('开发者自检'));
  expect(navigation.navigate).toHaveBeenLastCalledWith('DevSelfTest');
  expect(
    screen.getByText('Remote Screen 0.3.1 · 要求 Host ≥ 0.3.0'),
  ).toBeOnTheScreen();
});

it('opens the computer list from the paired computers row', async () => {
  const { navigation } = await renderScreen();
  // S16: the count leads somewhere.
  const row = screen.getByTestId('settings-paired-hosts');
  expect(row).toHaveProp('accessibilityRole', 'button');
  expect(row).toHaveProp('accessibilityHint', '回到首页查看电脑列表');
  await fireEvent.press(row);
  expect(navigation.navigate).toHaveBeenLastCalledWith('Home');
});

it('shows fixed facts without a value column or chevron', async () => {
  await renderScreen();
  // S20: always-on and single-choice rows read as facts, not choices.
  expect(screen.queryByText('始终开启')).toBeNull();
  expect(
    screen.getByText('始终开启：首次配对时记录，指纹变化会拒绝连接'),
  ).toBeOnTheScreen();
  expect(screen.queryByText('夜间')).toBeNull();
  expect(screen.getByText('目前只有夜间配色')).toBeOnTheScreen();
  expect(screen.queryByRole('button', { name: /^配色/ })).toBeNull();
  expect(screen.queryByRole('button', { name: /^证书指纹校验/ })).toBeNull();
});

it('steps cursor speed with spaced minus and plus buttons', async () => {
  await renderScreen();
  // S21: minus and plus, not arrows, with a gap between them.
  const slower = screen.getByLabelText('光标慢一点');
  const faster = screen.getByLabelText('光标快一点');
  // Minus draws one bar, plus two; neither is a chevron or text.
  expect(slower.children).toHaveLength(1);
  expect(faster.children).toHaveLength(2);
  expect(slower).toHaveTextContent('');
  expect(StyleSheet.flatten(slower.parent?.props.style)).toMatchObject({
    gap: 8,
  });
  expect(StyleSheet.flatten(slower.props.style)).toMatchObject({
    width: 44,
    height: 44,
  });
});

it('keeps the settings column readable on a wide screen', async () => {
  await renderScreen();
  // S31: an iPad gets a centred column instead of edge-to-edge rows.
  const scroll = screen.getByTestId('settings-scroll');
  expect(StyleSheet.flatten(scroll.props.contentContainerStyle)).toMatchObject({
    maxWidth: 680,
    alignSelf: 'center',
    width: '100%',
  });
});

it('keeps folded-away sessions 2 hours by default and can change it', async () => {
  const { settingsStore } = await renderScreen();

  expect(settingsStore.get('session.collapsedKeep')).toBe('2h');
  const row = screen.getByTestId('settings-collapsed-keep');
  expect(row).toHaveTextContent(/2 小时/);
  await fireEvent.press(row);
  expect(
    screen.getByText(copy.session.collapsedKeep.sheetTitle),
  ).toBeOnTheScreen();
  expect(screen.getByTestId('option-2h')).toBeSelected();
  for (const value of ['10m', '30m', 'always']) {
    expect(screen.getByTestId(`option-${value}`)).not.toBeSelected();
  }
  expect(screen.getByTestId('option-10m')).toHaveTextContent('10 分钟');
  await fireEvent.press(screen.getByTestId('option-always'));
  expect(settingsStore.get('session.collapsedKeep')).toBe('always');
  expect(screen.queryByText(copy.session.collapsedKeep.sheetTitle)).toBeNull();
  expect(screen.getByTestId('settings-collapsed-keep')).toHaveTextContent(
    /一直保持/,
  );
});

it('lists the session-switching shortcuts in the keyboard notes', async () => {
  await renderScreen();

  expect(screen.getByTestId('settings-session-shortcuts')).toHaveTextContent(
    copy.session.sessionShortcuts,
  );
  expect(copy.session.sessionShortcuts).toContain('三指左右滑');
  expect(copy.session.sessionShortcuts).toContain('⌃⌥⌘→');
  expect(copy.session.sessionShortcuts).toContain('⌃⌥⌘←');
  expect(copy.session.sessionShortcuts).toContain('⌃⌥⌘1…9');
  expect(copy.session.sessionShortcuts).toContain('⌃⌥⌘S');
});
