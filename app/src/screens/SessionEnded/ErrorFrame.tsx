import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import type { IconName } from '../../components/icons/Icon';
import { errorCodeLabel } from '../../features/format/misc';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { hit, tokens } from '../../theme/tokens';
import type { EndAction } from './boardTypes';

export type KickerTone = 'bad' | 'relay' | 'accent';

export type ErrorFrameProps = {
  kicker: string;
  kickerIcon?: IconName;
  kickerTone?: KickerTone;
  title: string;
  desc: string;
  illustration?: ReactNode;
  illustrationSummary?: string;
  extra?: ReactNode;
  meta?: string;
  code: number | null;
  onCopyCode: () => void;
  primary?: EndAction;
  secondary?: EndAction;
  more?: EndAction[];
  back?: EndAction;
  testID?: string;
};

// V27: phones in portrait and narrow split views get a slim gutter and the
// illustration above the text instead of beside it.
const WIDE_GUTTER = 59;
const NARROW_GUTTER = 16;
const NARROW_WIDTH = 600;
const ILLUSTRATION_WIDTH = 250;
const COLUMN_GAP = 32;
const MIN_TEXT_WIDTH = 260;

const kickerColors: Record<KickerTone, keyof typeof tokens> = {
  bad: 'bad',
  relay: 'relay',
  accent: 'accent',
};

export function ErrorFrame({
  kicker,
  kickerIcon,
  kickerTone = 'bad',
  title,
  desc,
  illustration,
  illustrationSummary,
  extra,
  meta,
  code,
  onCopyCode,
  primary,
  secondary,
  more = [],
  back,
  testID,
}: ErrorFrameProps) {
  const { width, height } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const gutter = width < NARROW_WIDTH ? NARROW_GUTTER : WIDE_GUTTER;
  const stacked =
    width - 2 * gutter < ILLUSTRATION_WIDTH + COLUMN_GAP + MIN_TEXT_WIDTH;
  const largeText = PixelRatio.getFontScale() > 2;
  const [availableHeight, setAvailableHeight] = useState(
    Math.max(0, height - 40 - 32),
  );
  const label = code === null ? null : errorCodeLabel(code);

  useEffect(() => {
    if (Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(title);
    }
  }, [title]);

  const showIllustration = illustration !== undefined && !largeText;

  return (
    <View style={styles.root} testID={testID}>
      <ScrollView
        contentContainerStyle={styles.scrollContent}
        onLayout={event => {
          setAvailableHeight(event.nativeEvent.layout.height);
        }}
        showsVerticalScrollIndicator={false}
        style={[
          styles.scroll,
          {
            top: Math.max(22, insets.top),
            right: Math.max(gutter, insets.right),
            bottom: Math.max(18, insets.bottom),
            left: Math.max(gutter, insets.left),
          },
        ]}
        testID="session-ended-scroll"
      >
        <View style={styles.content}>
          {!showIllustration ? (
            <View style={styles.singleColumn}>{renderColumn()}</View>
          ) : stacked ? (
            <View style={styles.stack}>
              <View
                accessibilityElementsHidden
                importantForAccessibility="no-hide-descendants"
                style={[styles.illustration, styles.illustrationStacked]}
                testID="session-ended-illustration"
              >
                {illustration}
              </View>
              {renderColumn()}
            </View>
          ) : (
            <View style={styles.columns}>
              <View
                accessibilityElementsHidden
                importantForAccessibility="no-hide-descendants"
                style={[
                  styles.illustration,
                  { height: Math.min(300, availableHeight) },
                ]}
                testID="session-ended-illustration"
              >
                {illustration}
              </View>
              <View style={[styles.column, styles.grow]}>{renderColumn()}</View>
            </View>
          )}
        </View>
      </ScrollView>
    </View>
  );

  function renderColumn() {
    return (
      <View style={styles.textColumn}>
        <View style={styles.kickerRow}>
          {kickerIcon === undefined ? null : (
            <Icon
              color={kickerColors[kickerTone]}
              name={kickerIcon}
              size={14}
            />
          )}
          <AppText
            color={kickerColors[kickerTone]}
            style={styles.kicker}
            weight={600}
          >
            {kicker}
          </AppText>
        </View>
        <AppText
          {...(Platform.OS === 'android'
            ? { accessibilityLiveRegion: 'polite' as const }
            : {})}
          accessibilityRole="header"
          style={styles.title}
          weight={600}
        >
          {title}
        </AppText>
        <AppText
          {...(largeText || illustrationSummary === undefined
            ? {}
            : {
                accessibilityLabel: sessionEnded.a11y.withSummary(
                  desc,
                  illustrationSummary,
                ),
              })}
          color="text2"
          style={styles.desc}
        >
          {desc}
        </AppText>
        {largeText && illustrationSummary !== undefined ? (
          <AppText color="text2" style={styles.desc}>
            {illustrationSummary}
          </AppText>
        ) : null}
        {extra}
        {meta === undefined && label === null ? null : (
          <View style={styles.metaRow}>
            {meta === undefined ? null : (
              <AppText color="text3" style={styles.meta}>
                {meta}
              </AppText>
            )}
            {meta !== undefined && label !== null ? (
              <AppText color="text3" style={styles.meta}>
                ·
              </AppText>
            ) : null}
            {label === null ? null : (
              <Pressable
                accessibilityLabel={sessionEnded.copyCode(label)}
                accessibilityRole="button"
                onPress={onCopyCode}
                style={({ pressed }) => [
                  styles.codeButton,
                  { minHeight: hit.min },
                  pressed ? styles.pressed : null,
                ]}
              >
                <View style={styles.codeButtonFrame}>
                  <AppText color="text2" mono style={styles.code}>
                    {label}
                  </AppText>
                </View>
              </Pressable>
            )}
          </View>
        )}
        {primary === undefined &&
        secondary === undefined &&
        more.length === 0 &&
        back === undefined ? null : (
          <View style={styles.actions}>
            {primary === undefined ? null : (
              <Button
                accessibilityHint={primary.accessibilityHint}
                disabled={primary.disabled}
                loading={primary.loading}
                onPress={primary.onPress}
                title={primary.label}
                variant="primary"
              />
            )}
            {secondary === undefined ? null : (
              <Button
                accessibilityHint={secondary.accessibilityHint}
                disabled={secondary.disabled}
                loading={secondary.loading}
                onPress={secondary.onPress}
                title={secondary.label}
                variant="secondary"
              />
            )}
            {more.map(action => (
              <Button
                accessibilityHint={action.accessibilityHint}
                disabled={action.disabled}
                key={action.label}
                loading={action.loading}
                onPress={action.onPress}
                title={action.label}
                variant="text"
              />
            ))}
            {back === undefined ? null : (
              <Button
                accessibilityHint={back.accessibilityHint}
                disabled={back.disabled}
                loading={back.loading}
                onPress={back.onPress}
                title={back.label}
                variant="text"
              />
            )}
          </View>
        )}
      </View>
    );
  }
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: tokens.thumbBg },
  scroll: { position: 'absolute' },
  scrollContent: { flexGrow: 1, justifyContent: 'center' },
  content: { width: '100%', justifyContent: 'center' },
  columns: { flexDirection: 'row', alignItems: 'center', gap: COLUMN_GAP },
  illustration: {
    width: ILLUSTRATION_WIDTH,
    overflow: 'hidden',
    borderRadius: 18,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
  },
  illustrationStacked: { width: '100%' },
  singleColumn: { width: '100%', maxWidth: 520 },
  stack: { width: '100%', maxWidth: 520, gap: 20 },
  column: { flex: 1, minWidth: 0 },
  grow: { flexGrow: 1 },
  textColumn: { gap: 12 },
  kickerRow: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  kicker: { fontSize: 12, lineHeight: 16 },
  title: { fontSize: 21, lineHeight: 28 },
  desc: { fontSize: 13, lineHeight: 20 },
  metaRow: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  meta: { fontSize: 12, lineHeight: 17 },
  codeButton: {
    justifyContent: 'center',
  },
  codeButtonFrame: {
    minHeight: 26,
    justifyContent: 'center',
    borderRadius: 8,
    borderWidth: 1,
    borderColor: tokens.inputLine,
    paddingHorizontal: 8,
  },
  code: { fontSize: 11.5, lineHeight: 15 },
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 8,
    marginTop: 4,
  },
  pressed: { opacity: 0.8 },
});

export default ErrorFrame;
