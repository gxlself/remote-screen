import {
  PixelRatio,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { coachContent } from '../../features/session/gestures';
import { useScreenReader } from '../../features/session/useScreenReader';
import { coach } from '../../i18n/zh/coach';
import { radius, space, tokens } from '../../theme/tokens';
import type { SessionCtx } from './slots';
import { useTitleFocus } from './useTitleFocus';

/** 低于这个高度（iPhone 横屏约 390）按紧凑排版。 */
const COMPACT_HEIGHT = 500;

export type GestureCoachProps = {
  ctx: SessionCtx;
  onClose?: () => void;
};

export function GestureCoach({ ctx, onClose }: GestureCoachProps) {
  const content = coachContent(ctx.mode, ctx.os);
  const close = onClose ?? ctx.closeCoach;
  const titleRef = useTitleFocus();
  const screenReaderEnabled = useScreenReader();
  const { height } = useWindowSize();
  // 大字号或 iPhone 横屏这类矮窗口时改成可滚动的紧凑排版，避免溢出。
  const scroll = PixelRatio.getFontScale() > 2 || height < COMPACT_HEIGHT;

  const instructions = (
    <>
      <View style={styles.header}>
        <View
          ref={titleRef as never}
          accessible
          accessibilityLabel={content.title.replace('：', '，')}
          accessibilityRole="header"
        >
          <AppText style={styles.title} variant="headline" weight={600}>
            {content.title}
          </AppText>
        </View>
        <AppText color="text2" variant="footnote">
          {content.body}
        </AppText>
      </View>
      <View style={scroll ? styles.gridLarge : styles.grid}>
        {[
          content.items.slice(0, Math.ceil(content.items.length / 2)),
          content.items.slice(Math.ceil(content.items.length / 2)),
        ].map(
          (row, rowIndex) => (
            <View
              key={rowIndex}
              style={[styles.row, scroll ? styles.rowLarge : null]}
            >
              {row.map(item => (
                <View
                  accessible
                  accessibilityLabel={coach.itemA11y(item.gesture, item.effect)}
                  key={item.gesture}
                  style={styles.cell}
                >
                  <View style={styles.glyph}>
                    <Icon color="text2" name={item.icon} size={28} />
                  </View>
                  <AppText variant="button">{item.gesture}</AppText>
                  <AppText color="text2" variant="caption">
                    {item.effect}
                  </AppText>
                </View>
              ))}
            </View>
          ),
        )}
      </View>
      <View style={styles.footer}>
        {content.footer.map(line => (
          <AppText color="text3" key={line} variant="caption">
            {line}
          </AppText>
        ))}
        {screenReaderEnabled ? (
          <AppText color="text3" variant="caption">
            {coach.screenReaderNote}
          </AppText>
        ) : null}
      </View>
    </>
  );

  return (
    <View
      accessibilityViewIsModal
      onAccessibilityEscape={close}
      style={styles.backdrop}
      testID="gesture-coach"
    >
      <View style={styles.content} testID="gesture-coach-content">
        {scroll ? (
          <ScrollView
            contentContainerStyle={styles.scrollContent}
            style={styles.scroll}
            testID="gesture-coach-scroll"
          >
            {instructions}
          </ScrollView>
        ) : (
          instructions
        )}
        <View style={styles.actions}>
          <Button
            accessibilityLabel={content.switchLabel}
            onPress={() => {
              ctx.setMode(content.switchTo);
              close();
            }}
            title={content.switchLabel}
            variant="secondary"
          />
          <Button
            accessibilityLabel={coach.start}
            onPress={close}
            title={coach.start}
            variant="primary"
          />
        </View>
      </View>
    </View>
  );
}

export default GestureCoach;

const styles = StyleSheet.create({
  backdrop: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    backgroundColor: 'rgba(0,0,0,0.7)',
  },
  content: {
    position: 'absolute',
    left: 59,
    right: 59,
    top: 16,
    bottom: 14,
    gap: space.md,
  },
  header: { gap: space.md },
  title: { fontSize: 17, lineHeight: 22 },
  scroll: { flexShrink: 1 },
  scrollContent: { gap: space.md },
  grid: { flex: 1, gap: 10 },
  gridLarge: { gap: 10 },
  row: { flex: 1, flexDirection: 'row', gap: 10 },
  rowLarge: { flex: 0 },
  cell: {
    flex: 1,
    padding: space.md,
    gap: 4,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
  },
  glyph: { height: 40, justifyContent: 'center' },
  footer: { gap: 2 },
  actions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    gap: space.sm,
  },
});
