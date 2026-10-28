import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import { PixelRatio, Platform, StyleSheet } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { netTailnetOk } from '../../native/rscore/fixtures';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SettingsScreen from '.';

jest.setTimeout(30000);

async function renderSettings(opts: { tailnetAvailable?: boolean } = {}) {
  const core = createFakeCore({
    network: netTailnetOk,
    tailnetAvailable: opts.tailnetAvailable ?? false,
  });
  const platform = createFakePlatform();
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Settings'>['navigation'];
  const rendered = await renderWithProviders(
    <SettingsScreen
      navigation={navigation}
      route={{ key: 'Settings-a11y', name: 'Settings' }}
    />,
    { core, platform, settings: { 'tailnet.enabled': true } },
  );
  await act(() => Promise.resolve());
  return rendered;
}

afterEach(() => {
  jest.restoreAllMocks();
});

it('marks quality options as radio buttons with checked and selected state', async () => {
  await renderSettings();
  await fireEvent.press(screen.getByLabelText('画质，自动'));

  const clear = screen.getByRole('radio', { name: '清晰' });
  expect(clear).toHaveProp('accessibilityState', {
    checked: false,
    selected: false,
  });
  await fireEvent.press(clear);
  await fireEvent.press(screen.getByLabelText('画质，清晰'));

  expect(screen.getByRole('radio', { name: '清晰' })).toHaveProp(
    'accessibilityState',
    { checked: true, selected: true },
  );
});

it('exposes the cursor speed row as adjustable and keeps increment working', async () => {
  const rendered = await renderSettings();
  const adjustable = screen.getByLabelText('光标速度');

  expect(adjustable).toHaveProp('accessible', true);
  expect(adjustable).toHaveProp('accessibilityRole', 'adjustable');
  await fireEvent(adjustable, 'accessibilityAction', {
    nativeEvent: { actionName: 'increment' },
  });
  expect(rendered.settingsStore.get('session.cursorSpeed')).toBe(1.6);
});

it('uses the Android touch and natural-scroll copy only on Android', async () => {
  await renderSettings();
  expect(
    screen.getByText(
      '整块屏幕都是触控板，单指移动光标、轻点单击。适合手机这种小屏。',
    ),
  ).toBeOnTheScreen();
  expect(
    screen.getByText('双指往上推，页面往上走，和 iPhone 一样'),
  ).toBeOnTheScreen();
  expect(screen.getByText('iPhone 同时只能开一个 VPN')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('触屏直点'));
  expect(
    screen.getByText('手指点到哪里就点哪里，拖动会移动窗口。适合 iPad 大屏。'),
  ).toBeOnTheScreen();

  await cleanup();
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderSettings();
  await fireEvent.press(screen.getByLabelText('触屏直点'));
  expect(
    screen.getByText('手指点到哪里就点哪里，拖动会移动窗口。适合平板大屏。'),
  ).toBeOnTheScreen();
  expect(
    screen.getByText('双指往上推，页面往上走，和手机上滑动一样'),
  ).toBeOnTheScreen();
  expect(
    screen.getByText('Android 同时只能开一个 VPN，工作资料里的不算'),
  ).toBeOnTheScreen();
  expect(screen.queryByText('和 iPhone 一样', { exact: false })).toBeNull();
  expect(screen.queryByText('iPhone 同时只能开一个 VPN')).toBeNull();
});

it.each([1, 2, 3])(
  'stacks the Tailscale mode control under its title at font scale %s',
  async scale => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(scale);
    await renderSettings({ tailnetAvailable: true });

    // S04: the three modes get the whole row; nothing is squeezed to 230.
    const title = await screen.findByText('接入方式');
    const group = screen.getByLabelText('接入 Tailscale 的方式');
    expect(title.parent).toBe(group.parent);
    expect(StyleSheet.flatten(group.parent?.props.style).width).toBeUndefined();
    expect(screen.getByLabelText('App 内连接')).toBeOnTheScreen();
    expect(screen.queryByText('App 内连接在后续版本提供')).toBeNull();
  },
);

it('reads the description of each defaults switch as its hint', async () => {
  await renderSettings();
  // S22
  expect(screen.getByLabelText('点击时轻震')).toHaveProp(
    'accessibilityHint',
    '单击、拖拽开始时给一下触感反馈',
  );
  expect(screen.getByLabelText('自动接回 tmux')).toHaveProp(
    'accessibilityHint',
    '打开终端时进入 tmux，关掉再打开能回到原来的会话',
  );
});
