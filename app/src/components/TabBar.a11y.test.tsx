import type { BottomTabBarProps } from '@react-navigation/bottom-tabs';
import { screen } from '@testing-library/react-native';
import { PixelRatio, Platform, processColor, StyleSheet } from 'react-native';
import { SafeAreaInsetsContext } from 'react-native-safe-area-context';
import { mockNavigation } from '../test/navigation';
import { renderWithProviders } from '../test/render';
import { hit, tokens } from '../theme/tokens';
import { textStyle } from '../theme/typography';
import TabBar from './TabBar';

async function mount(bottom = 0) {
  const routes = ['Home', 'Sessions', 'Files', 'Settings'].map(name => ({
    key: `${name}-key`,
    name,
  }));
  const props = {
    state: { index: 0, routes },
    descriptors: Object.fromEntries(
      routes.map(route => [
        route.key,
        { options: { tabBarBadge: route.name === 'Sessions' ? 3 : undefined } },
      ]),
    ),
    navigation: mockNavigation<'MainTabs'>(),
  } as unknown as BottomTabBarProps;

  return renderWithProviders(
    <SafeAreaInsetsContext.Provider value={{ top: 0, right: 0, bottom, left: 0 }}>
      <TabBar {...props} />
    </SafeAreaInsetsContext.Provider>,
  );
}

function barStyle() {
  return StyleSheet.flatten(
    screen.getByRole('tab', { name: '设备' }).parent?.props.style,
  );
}

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
});

afterEach(() => {
  jest.restoreAllMocks();
});

describe('TabBar platform layout and accessibility', () => {
  it('uses the Android navigation bar and 64 by 32 selection indicators', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await mount();

    expect(barStyle()).toMatchObject({
      minHeight: 80,
      paddingTop: 12,
      paddingBottom: 16,
    });
    expect(barStyle().height).toBeUndefined();
    expect(
      StyleSheet.flatten(screen.getByTestId('tab-indicator-Home').props.style),
    ).toMatchObject({
      width: 64,
      height: 32,
      borderRadius: 16,
      backgroundColor: tokens.accentSoft,
      alignItems: 'center',
      justifyContent: 'center',
    });
    expect(
      StyleSheet.flatten(screen.getByTestId('tab-indicator-Sessions').props.style)
        .backgroundColor,
    ).toBe('transparent');
    expect(StyleSheet.flatten(screen.getByText('设备').props.style)).toMatchObject({
      ...textStyle('caption', 600),
      color: tokens.accent,
    });
    expect(StyleSheet.flatten(screen.getByText('会话').props.style)).toMatchObject({
      ...textStyle('caption', 500),
      color: tokens.text3,
    });
    expect(screen.getByRole('tab', { name: '设备' })).toHaveProp(
      'accessibilityState',
      expect.objectContaining({ selected: true }),
    );
    expect(screen.getByRole('tab', { name: '会话，3 个进行中' })).toHaveProp(
      'accessibilityState',
      expect.objectContaining({ selected: false }),
    );
  });

  it('adds the Android bottom safe area to the height and padding', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await mount(24);
    expect(barStyle()).toMatchObject({ minHeight: 104, paddingBottom: 40 });
  });

  it('keeps the iOS structure and typography with a flexible bar height', async () => {
    await mount();
    expect(screen.queryAllByTestId(/^tab-indicator-/)).toHaveLength(0);
    expect(barStyle()).toMatchObject({
      minHeight: 84,
      paddingTop: 6,
      paddingBottom: 8,
    });
    expect(barStyle().height).toBeUndefined();
    expect(StyleSheet.flatten(screen.getByText('设备').props.style)).toMatchObject(
      textStyle('tiny', 600),
    );
  });

  it.each(['ios', 'android'] as const)(
    'reads hit.min during rendering and uses platform press feedback on %s',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await mount();
      const tabs = screen.getAllByRole('tab');
      expect(tabs).toHaveLength(4);
      for (const tab of tabs) {
        expect(StyleSheet.flatten(tab.props.style)).toMatchObject({
          minHeight: hit.min,
          justifyContent: os === 'android' ? 'flex-start' : 'center',
          gap: os === 'android' ? 4 : 3,
        });
        // Pressable converts android_ripple into the native foreground prop.
        if (os === 'android') {
          expect(tab).toHaveProp(
            'nativeForegroundAndroid',
            expect.objectContaining({
              type: 'RippleAndroid',
              color: processColor('rgba(255, 255, 255, 0.12)'),
            }),
          );
        } else {
          expect(tab.props.nativeForegroundAndroid).toBeUndefined();
          expect(tab.props.nativeBackgroundAndroid).toBeUndefined();
        }
      }
    },
  );

  it.each(['ios', 'android'] as const)(
    'allows the bar and badge to grow at font scale 3 on %s',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
      await mount();
      expect(barStyle().height).toBeUndefined();
      const badge = StyleSheet.flatten(screen.getByText('3').parent?.props.style);
      expect(badge).toMatchObject({ minHeight: 18, top: -7, right: -10 });
      expect(badge.height).toBeUndefined();
      expect(screen.getByText('设备').props.numberOfLines).toBeUndefined();
      expect(screen.getByText('3').props.numberOfLines).toBeUndefined();
    },
  );
});
