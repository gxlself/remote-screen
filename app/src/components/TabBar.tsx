import type { BottomTabBarProps } from '@react-navigation/bottom-tabs';
import { Platform, Pressable, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { common } from '../i18n/zh/common';
import { hit, tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon, type IconName } from './icons/Icon';

type TabMeta = { icon: IconName; label: string };

const TABS: Record<string, TabMeta> = {
  Home: { icon: 'computer', label: common.tabs.devices },
  Sessions: { icon: 'sessions', label: common.tabs.sessions },
  Files: { icon: 'folder', label: common.tabs.files },
  Settings: { icon: 'quality', label: common.tabs.settings },
};

const BAR_BACKGROUND = 'rgba(13, 17, 20, 0.9)';
const BAR_LINE = '#1E262C';
// 同 `ripple.neutral`。
const RIPPLE = 'rgba(255, 255, 255, 0.12)';

export default function TabBar({
  state,
  descriptors,
  navigation,
}: BottomTabBarProps) {
  const insets = useSafeAreaInsets();
  const android = Platform.OS === 'android';

  return (
    <View
      accessibilityRole="tablist"
      style={[
        styles.bar,
        android ? styles.androidBar : null,
        android
          ? {
              // 07 · Android：导航栏 80，加底部安全区。
              minHeight: 80 + insets.bottom,
              paddingBottom: insets.bottom + 16,
            }
          : { paddingBottom: Math.max(insets.bottom, 8) },
      ]}
    >
      {state.routes.map((route, index) => {
        const { options } = descriptors[route.key];
        const selected = state.index === index;
        const meta = TABS[route.name] ?? {
          icon: 'computer' as const,
          label: options.title ?? route.name,
        };
        const badge =
          typeof options.tabBarBadge === 'number' && options.tabBarBadge > 0
            ? options.tabBarBadge
            : undefined;
        const label =
          badge && route.name === 'Sessions'
            ? `${meta.label}，${common.a11y.sessionsBadge(badge)}`
            : meta.label;
        const icon = (
          <View style={styles.iconWrap}>
            <Icon
              name={meta.icon}
              size={24}
              color={selected ? 'accent' : 'text3'}
            />
            {badge == null ? null : (
              <View style={styles.badge}>
                <AppText color="accentText" style={styles.badgeText} weight={700}>
                  {String(badge)}
                </AppText>
              </View>
            )}
          </View>
        );

        return (
          <Pressable
            key={route.key}
            accessibilityRole="tab"
            accessibilityState={{ selected }}
            accessibilityLabel={label}
            testID={options.tabBarButtonTestID}
            android_ripple={
              android ? { color: RIPPLE, foreground: true } : undefined
            }
            style={[
              styles.button,
              { minHeight: hit.min },
              android ? styles.androidButton : null,
            ]}
            onPress={() => {
              const event = navigation.emit({
                type: 'tabPress',
                target: route.key,
                canPreventDefault: true,
              });
              if (!selected && !event.defaultPrevented) {
                navigation.navigate(route.name);
              }
            }}
            onLongPress={() => {
              navigation.emit({ type: 'tabLongPress', target: route.key });
            }}
          >
            {android ? (
              <View
                style={[
                  styles.indicator,
                  selected ? styles.selectedIndicator : null,
                ]}
                testID={`tab-indicator-${route.name}`}
              >
                {icon}
              </View>
            ) : (
              icon
            )}
            <AppText
              color={selected ? 'accent' : 'text3'}
              variant={android ? 'caption' : 'tiny'}
              weight={selected ? 600 : 500}
            >
              {meta.label}
            </AppText>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  bar: {
    minHeight: 84,
    paddingTop: 6,
    flexDirection: 'row',
    backgroundColor: BAR_BACKGROUND,
    borderTopColor: BAR_LINE,
    borderTopWidth: 1,
  },
  button: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    gap: 3,
  },
  androidBar: {
    // 07 · Android：M3 上 / 下内边距 12 / 16，下边距另加安全区。
    paddingTop: 12,
  },
  androidButton: {
    justifyContent: 'flex-start',
    gap: 4,
  },
  indicator: {
    // 07 · Android：64×32 选中胶囊。
    width: 64,
    height: 32,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'transparent',
  },
  selectedIndicator: {
    backgroundColor: tokens.accentSoft,
  },
  iconWrap: {
    width: 24,
    height: 24,
  },
  badge: {
    position: 'absolute',
    top: -7,
    right: -10,
    minWidth: 18,
    minHeight: 18,
    borderRadius: 9,
    paddingHorizontal: 4,
    backgroundColor: tokens.accent,
    alignItems: 'center',
    justifyContent: 'center',
  },
  badgeText: {
    fontSize: 11,
    lineHeight: 14,
  },
});
