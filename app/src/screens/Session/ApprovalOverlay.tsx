import {
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Countdown, useCountdown } from '../../components/Countdown';
import { Icon } from '../../components/icons/Icon';
import { common } from '../../i18n/zh/common';
import { sessionConnect } from '../../i18n/zh/sessionConnect';
import { radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';
import { useTitleFocus } from './useTitleFocus';

const OVERLAY = 'rgba(0,0,0,0.6)';
const PANEL_MAX_WIDTH = 452;
const PANEL_SIDE_INSET = 59;

export function ApprovalOverlay({ ctx }: SlotProps) {
  const { width, height: windowHeight } = useWindowSize();
  const timeoutS = ctx.rec?.approvalTimeoutS ?? 30;
  const remaining = useCountdown(timeoutS, {
    running: true,
    key: ctx.sessionId,
  });
  const title =
    ctx.os === 'windows'
      ? sessionConnect.approval.windowsTitle
      : sessionConnect.approval.macTitle;
  const panelWidth = Math.min(
    PANEL_MAX_WIDTH,
    Math.max(0, width - PANEL_SIDE_INSET * 2),
  );
  const titleRef = useTitleFocus();

  return (
    <View
      accessibilityViewIsModal
      style={styles.root}
      testID="approval-overlay"
    >
      <View
        style={[
          styles.panel,
          { width: panelWidth, maxHeight: windowHeight - 24 },
        ]}
        testID="approval-panel"
      >
        <ScrollView
          contentContainerStyle={styles.content}
          style={styles.scroll}
        >
          <View style={styles.header}>
            <View style={styles.iconCircle}>
              <Icon color="accent" name="hand" size={18} />
            </View>
            <View style={styles.headerCopy}>
              <View
                ref={titleRef as never}
                accessible
                accessibilityLabel={title}
                accessibilityRole="header"
              >
                <AppText style={styles.title} variant="headline" weight={600}>
                  {title}
                </AppText>
              </View>
            </View>
            <Countdown remaining={remaining} total={timeoutS} />
          </View>

          <AppText color="text2" style={styles.body} variant="body">
            {sessionConnect.approval.body(ctx.hostName)}
          </AppText>
        </ScrollView>

        <View style={styles.footer}>
          <Button
            accessibilityLabel={common.cancel}
            onPress={ctx.cancelConnect}
            title={common.cancel}
            variant="secondary"
          />
        </View>
      </View>
    </View>
  );
}

export default ApprovalOverlay;

const styles = StyleSheet.create({
  root: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: PANEL_SIDE_INSET,
    backgroundColor: OVERLAY,
  },
  panel: {
    paddingTop: 18,
    paddingHorizontal: 18,
    paddingBottom: 14,
    borderRadius: radius.panel,
    borderWidth: 1,
    borderColor: tokens.panelLine,
    backgroundColor: tokens.panel,
    shadowColor: '#000',
    shadowOffset: { width: 0, height: 12 },
    shadowOpacity: 0.45,
    shadowRadius: 28,
    elevation: 24,
  },
  scroll: { flexShrink: 1 },
  content: { gap: 14 },
  header: { flexDirection: 'row', alignItems: 'center', gap: 12 },
  iconCircle: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  headerCopy: { flex: 1, minWidth: 0 },
  title: { flexShrink: 1 },
  body: { lineHeight: 22 },
  footer: { flexDirection: 'row', justifyContent: 'flex-end' },
});
