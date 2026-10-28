import {
  PixelRatio,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Countdown, useCountdown } from '../../components/Countdown';
import { useSessionsActions } from '../../features/session/store';
import { common } from '../../i18n/zh/common';
import { localIdleReconnect } from '../../i18n/zh/localIdleReconnect';
import { useCore } from '../../native/rscore/CoreContext';
import { radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';
import { useTitleFocus } from './useTitleFocus';

const COPY = localIdleReconnect.idle;
const LEFT_SHIFT_USAGE = 0xe1;
const DIALOG_MAX_WIDTH = 400;

export function IdleOverlay({ ctx }: SlotProps) {
  const { width: windowWidth, height: windowHeight } = useWindowSize();
  const warning = ctx.rec?.idleWarning ?? null;
  const remaining = useCountdown(warning?.secondsLeft ?? 0, {
    running: warning !== null,
    key: warning?.atUs,
  });
  const core = useCore();
  const { clearIdleWarning } = useSessionsActions();
  const titleRef = useTitleFocus(warning !== null);

  if (warning === null) {
    return null;
  }

  const continueUsing = () => {
    const id = ctx.sessionId;
    if (id === null) {
      return;
    }
    core.sessions.key(id, LEFT_SHIFT_USAGE, true, 0);
    core.sessions.key(id, LEFT_SHIFT_USAGE, false, 0);
    clearIdleWarning(id);
  };

  const description =
    COPY.description[ctx.os][warning.lockOnEnd ? 'locked' : 'unlocked'];

  return (
    <View style={styles.overlay} testID="idle-overlay">
      <View
        accessibilityViewIsModal
        style={[
          styles.dialog,
          {
            // 分屏或窄窗口时两边至少留 16，不溢出。
            width: Math.min(DIALOG_MAX_WIDTH, Math.max(0, windowWidth - 32)),
            maxHeight: windowHeight - 24,
          },
        ]}
        testID="idle-dialog"
      >
        <ScrollView
          contentContainerStyle={styles.content}
          style={styles.scroll}
        >
          <View
            style={[
              styles.summary,
              PixelRatio.getFontScale() > 2 ? styles.summaryLarge : null,
            ]}
          >
            <Countdown
              remaining={remaining}
              size={60}
              total={warning.secondsLeft}
            />
            <View style={styles.copy}>
              <View
                ref={titleRef as never}
                accessible
                accessibilityLabel={COPY.title}
                accessibilityRole="header"
              >
                <AppText variant="headline" weight={600}>
                  {COPY.title}
                </AppText>
              </View>
              <AppText color="text2" variant="footnote">
                {description}
              </AppText>
            </View>
          </View>
        </ScrollView>
        <View style={styles.actions}>
          <Button
            accessibilityLabel={common.disconnect}
            onPress={ctx.disconnect}
            title={common.disconnect}
            variant="secondary"
          />
          <Button
            accessibilityLabel={COPY.continueUsing}
            onPress={continueUsing}
            title={COPY.continueUsing}
            variant="primary"
          />
        </View>
      </View>
    </View>
  );
}

export default IdleOverlay;

const styles = StyleSheet.create({
  overlay: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(0,0,0,0.62)',
  },
  dialog: {
    paddingTop: 18,
    paddingHorizontal: 18,
    paddingBottom: 14,
    borderRadius: radius.panel,
    borderWidth: 1,
    borderColor: tokens.panelLine,
    backgroundColor: tokens.panel,
  },
  scroll: { flexShrink: 1 },
  content: { gap: 18 },
  summary: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 16,
  },
  summaryLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  copy: {
    flex: 1,
    gap: 5,
  },
  actions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: 10,
  },
});
