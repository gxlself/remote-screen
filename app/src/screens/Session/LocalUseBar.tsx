import { useEffect, useRef } from 'react';
import {
  AccessibilityInfo,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { localIdleReconnect } from '../../i18n/zh/localIdleReconnect';
import { radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';

const COPY = localIdleReconnect.localUse;

export function LocalUseBar({ ctx }: SlotProps) {
  const active = ctx.view.localUse;
  const announced = useRef(false);
  const { width } = useWindowSize();

  useEffect(() => {
    if (Platform.OS === 'ios' && active && !announced.current) {
      AccessibilityInfo.announceForAccessibility(COPY.title);
    }
    announced.current = active;
  }, [active]);

  if (!active) {
    return null;
  }

  const barWidth = Math.max(0, Math.min(396, width - 2 * 59));

  return (
    // 放在会话页顶部的提示栏里，由提示栏负责位置（R37）。
    <View pointerEvents="none">
      <View
        accessibilityLiveRegion="polite"
        style={[styles.bar, { width: barWidth }]}
        testID="local-use-bar"
      >
        <View style={styles.iconWell}>
          <Icon color="relay" name="hand" size={16} />
        </View>
        <View style={styles.copy}>
          <AppText variant="footnote" weight={600}>
            {COPY.title}
          </AppText>
          <AppText color="text2" variant="caption">
            {COPY.hint}
          </AppText>
        </View>
      </View>
    </View>
  );
}

export default LocalUseBar;

const styles = StyleSheet.create({
  bar: {
    minHeight: 58,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingTop: 10,
    paddingRight: 14,
    paddingBottom: 10,
    paddingLeft: 10,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: 'rgba(244,178,78,0.35)',
    backgroundColor: 'rgba(28,24,16,0.95)',
  },
  iconWell: {
    width: 28,
    height: 28,
    flexShrink: 0,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 14,
    backgroundColor: tokens.relaySoft,
  },
  copy: {
    flex: 1,
    gap: 1,
  },
});
