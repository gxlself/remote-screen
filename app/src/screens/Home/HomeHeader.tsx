import {
  AccessibilityInfo,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import { useEffect, useRef } from 'react';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { IconButton } from '../../components/IconButton';
import { common } from '../../i18n/zh/common';
import { home } from '../../i18n/zh/home';
import { hit, tokens } from '../../theme/tokens';
import type { HomeStatus, HomeStatusTone } from './useHomeModel';

export type HomeHeaderProps = {
  status: HomeStatus;
  onAdd: () => void;
  onSettings: () => void;
};

const statusColors = {
  good: tokens.good,
  warn: tokens.relay,
  unknown: tokens.text3,
} as const;

const statusHalos = {
  good: 'rgba(93,211,154,0.18)',
  warn: 'rgba(244,178,78,0.18)',
  unknown: 'rgba(134,146,154,0.18)',
} as const;

export function HomeHeader({ status, onAdd, onSettings }: HomeHeaderProps) {
  const previousTone = useRef<HomeStatusTone | null>(null);

  useEffect(() => {
    const previous = previousTone.current;
    if (
      Platform.OS === 'ios' &&
      previous !== null &&
      previous !== 'unknown' &&
      previous !== status.tone
    ) {
      AccessibilityInfo.announceForAccessibility(status.text);
    }
    previousTone.current = status.tone;
  }, [status.text, status.tone]);

  return (
    <View
      style={[
        styles.header,
        Platform.OS === 'android' ? styles.androidHeader : null,
      ]}
    >
      <View style={styles.copy}>
        <AppText accessibilityRole="header" variant="largeTitle">
          {home.title}
        </AppText>
        <View style={styles.statusRow}>
          <View
            style={[
              styles.statusHalo,
              { backgroundColor: statusHalos[status.tone] },
            ]}
          >
            <View
              style={[
                styles.statusDot,
                { backgroundColor: statusColors[status.tone] },
              ]}
              testID="home-status-dot"
            />
          </View>
          <AppText
            accessibilityLiveRegion={
              Platform.OS === 'android' ? 'polite' : undefined
            }
            color="text2"
            variant="footnote"
          >
            {status.text}
          </AppText>
        </View>
      </View>
      <View style={styles.actions}>
        <IconButton
          accessibilityLabel={common.a11y.add}
          icon="plus"
          onPress={onAdd}
          size={44}
          testID="home-add"
          variant="raised"
        />
        <Pressable
          accessibilityLabel={common.a11y.settings}
          accessibilityRole="button"
          onPress={onSettings}
          style={[
            styles.avatar,
            {
              width: hit.min,
              height: hit.min,
              borderRadius: hit.min / 2,
            },
          ]}
          testID="home-settings"
        >
          <Icon color="accent" name="user" size={20} />
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    justifyContent: 'space-between',
  },
  androidHeader: {
    // 同 07 · Android
    minHeight: 64,
  },
  copy: {
    flex: 1,
    gap: 4,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  statusHalo: {
    width: 14,
    height: 14,
    borderRadius: 7,
    alignItems: 'center',
    justifyContent: 'center',
  },
  statusDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
  },
  actions: {
    flexDirection: 'row',
    gap: 8,
  },
  avatar: {
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accentSoft,
  },
});
