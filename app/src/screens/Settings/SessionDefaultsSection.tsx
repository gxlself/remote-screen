import { useState } from 'react';
import { Platform, Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { Switch } from '../../components/Switch';
import { useSetting } from '../../features/settings/store';
import type {
  CollapsedKeep,
  SessionMode,
} from '../../features/settings/schema';
import { settings as copy } from '../../i18n/zh/settings';
import { usePlatform } from '../../native/platform/PlatformContext';
import { isAndroid, press, ripple } from '../../theme/platform';
import { hit, radius, space, tokens } from '../../theme/tokens';
import { OptionSheet } from './OptionSheet';

const QUALITY_OPTIONS = [
  {
    value: 'auto',
    label: copy.session.quality.value.auto,
    subtitle: copy.session.quality.optionSubtitle.auto,
  },
  {
    value: 'smooth',
    label: copy.session.quality.value.smooth,
    subtitle: copy.session.quality.optionSubtitle.smooth,
  },
  {
    value: 'clear',
    label: copy.session.quality.value.clear,
    subtitle: copy.session.quality.optionSubtitle.clear,
  },
  {
    value: 'eco',
    label: copy.session.quality.value.eco,
    subtitle: copy.session.quality.optionSubtitle.eco,
  },
] as const;

const KEEP_LABELS = copy.session.collapsedKeep.value;
const KEEP_OPTIONS: { value: CollapsedKeep; label: string }[] = [
  { value: '10m', label: KEEP_LABELS['10m'] },
  { value: '30m', label: KEEP_LABELS['30m'] },
  { value: '2h', label: KEEP_LABELS['2h'] },
  { value: 'always', label: KEEP_LABELS.always },
];

const MODE_OPTIONS = [
  { value: 'trackpad' as const, label: copy.session.mode.trackpad },
  { value: 'touch' as const, label: copy.session.mode.touch },
];

function touchHint(): string {
  return Platform.OS === 'android'
    ? copy.session.mode.touchHintAndroid
    : copy.session.mode.touchHint;
}

function naturalScrollSubtitle(): string {
  return Platform.OS === 'android'
    ? copy.session.naturalScroll.subtitleAndroid
    : copy.session.naturalScroll.subtitle;
}

const SPEED_MIN = 0.6;
const SPEED_MAX = 2.4;
const SPEED_STEP = 0.2;

function roundSpeed(value: number): number {
  return Math.round(value * 10) / 10;
}

type Props = {
  qualitySheetOpen: boolean;
  onOpenQualitySheet: () => void;
  onCloseQualitySheet: () => void;
};

export function SessionDefaultsSection({
  qualitySheetOpen,
  onOpenQualitySheet,
  onCloseQualitySheet,
}: Props) {
  const platform = usePlatform();
  const isPad = platform.deviceInfo().isPad;
  const modeKey = isPad ? 'session.mode.tablet' : 'session.mode.phone';
  const [rawMode, setMode] = useSetting(modeKey);
  const mode: SessionMode = rawMode;
  const [quality, setQuality] = useSetting('quality.default');
  const [cursorSpeedValue, setCursorSpeedRaw] = useSetting(
    'session.cursorSpeed',
  );
  const cursorSpeed: number = cursorSpeedValue;
  const setCursorSpeed = setCursorSpeedRaw as (value: number) => void;
  const [naturalScroll, setNaturalScroll] = useSetting('session.naturalScroll');
  const [haptics, setHaptics] = useSetting('session.haptics');
  const [sound, setSound] = useSetting('session.sound');
  const [pasteSync, setPasteSync] = useSetting('keyboard.pasteSync');
  const [keep, setKeep] = useSetting('session.collapsedKeep');
  const [keepSheetOpen, setKeepSheetOpen] = useState(false);

  const updateSpeed = (delta: number) => {
    const next = roundSpeed(
      Math.min(SPEED_MAX, Math.max(SPEED_MIN, cursorSpeed + delta)),
    );
    if (next !== cursorSpeed) {
      setCursorSpeed(next);
    }
  };

  return (
    <View style={styles.section}>
      <SectionHeader title={copy.session.section} />
      <ListGroup>
        <View style={styles.modeRow}>
          <AppText variant="body">{copy.session.mode.title}</AppText>
          <Segmented
            accessibilityLabel={copy.session.mode.title}
            onChange={setMode}
            options={MODE_OPTIONS}
            value={mode}
          />
          <AppText color="text3" style={styles.hint} variant="footnote">
            {mode === 'trackpad' ? copy.session.mode.trackpadHint : touchHint()}
          </AppText>
        </View>
        <ListRow
          chevron
          onPress={onOpenQualitySheet}
          title={copy.session.quality.title}
          value={copy.session.quality.value[quality]}
        />
        <View
          accessible
          accessibilityActions={[
            { name: 'increment', label: copy.session.cursorSpeed.faster },
            { name: 'decrement', label: copy.session.cursorSpeed.slower },
          ]}
          accessibilityLabel={copy.session.cursorSpeed.title}
          accessibilityRole="adjustable"
          accessibilityValue={{
            text: copy.session.cursorSpeed.spoken(cursorSpeed),
          }}
          onAccessibilityAction={event => {
            if (event.nativeEvent.actionName === 'increment') {
              updateSpeed(SPEED_STEP);
            } else if (event.nativeEvent.actionName === 'decrement') {
              updateSpeed(-SPEED_STEP);
            }
          }}
          style={styles.speedRow}
        >
          <View style={styles.speedCopy}>
            <View style={styles.speedTitleRow}>
              <AppText variant="body">{copy.session.cursorSpeed.title}</AppText>
              <AppText color="text2" variant="body">
                {copy.session.cursorSpeed.value(cursorSpeed)}
              </AppText>
            </View>
            <AppText color="text3" variant="caption">
              {copy.session.cursorSpeed.hint}
            </AppText>
          </View>
          <View style={styles.speedButtons}>
            {/* S21: minus and plus, not arrows, with room between them. */}
            <StepButton
              accessibilityLabel={copy.a11y.cursorSlower}
              disabled={cursorSpeed <= SPEED_MIN}
              glyph="minus"
              onPress={() => updateSpeed(-SPEED_STEP)}
            />
            <StepButton
              accessibilityLabel={copy.a11y.cursorFaster}
              disabled={cursorSpeed >= SPEED_MAX}
              glyph="plus"
              onPress={() => updateSpeed(SPEED_STEP)}
            />
          </View>
        </View>
        <ListRow
          right={
            <Switch
              accessibilityHint={naturalScrollSubtitle()}
              accessibilityLabel={copy.session.naturalScroll.title}
              onValueChange={setNaturalScroll}
              value={naturalScroll}
            />
          }
          subtitle={naturalScrollSubtitle()}
          title={copy.session.naturalScroll.title}
        />
        <ListRow
          right={
            <Switch
              accessibilityHint={copy.session.haptics.subtitle}
              accessibilityLabel={copy.session.haptics.title}
              onValueChange={setHaptics}
              value={haptics}
            />
          }
          subtitle={copy.session.haptics.subtitle}
          title={copy.session.haptics.title}
        />
        <ListRow
          right={
            <Switch
              accessibilityHint={copy.session.sound.subtitle}
              accessibilityLabel={copy.session.sound.title}
              onValueChange={setSound}
              value={sound}
            />
          }
          subtitle={copy.session.sound.subtitle}
          title={copy.session.sound.title}
        />
        <ListRow
          right={
            <Switch
              accessibilityHint={copy.session.pasteSync.subtitle}
              accessibilityLabel={copy.session.pasteSync.title}
              onValueChange={setPasteSync}
              testID="settings-paste-sync"
              value={pasteSync}
            />
          }
          subtitle={copy.session.pasteSync.subtitle}
          title={copy.session.pasteSync.title}
        />
        <ListRow
          chevron
          onPress={() => setKeepSheetOpen(true)}
          subtitle={copy.session.collapsedKeep.subtitle}
          testID="settings-collapsed-keep"
          title={copy.session.collapsedKeep.title}
          value={copy.session.collapsedKeep.value[keep]}
        />
      </ListGroup>
      {isPad ? (
        <AppText
          color="text3"
          style={styles.footnote}
          testID="settings-command-period"
          variant="footnote"
        >
          {copy.session.commandPeriod}
        </AppText>
      ) : null}
      <AppText
        color="text3"
        style={styles.footnote}
        testID="settings-session-shortcuts"
        variant="footnote"
      >
        {copy.session.sessionShortcuts}
      </AppText>
      {keepSheetOpen ? (
        <OptionSheet
          onClose={() => setKeepSheetOpen(false)}
          onSelect={setKeep}
          options={KEEP_OPTIONS}
          title={copy.session.collapsedKeep.sheetTitle}
          value={keep}
        />
      ) : null}
      {qualitySheetOpen ? (
        <OptionSheet
          onClose={onCloseQualitySheet}
          onSelect={setQuality}
          options={[...QUALITY_OPTIONS]}
          title={copy.session.quality.sheetTitle}
          value={quality}
        />
      ) : null}
    </View>
  );
}

function StepButton({
  accessibilityLabel,
  disabled,
  glyph,
  onPress,
}: {
  accessibilityLabel: string;
  disabled: boolean;
  glyph: 'minus' | 'plus';
  onPress: () => void;
}) {
  // Drawn bars, not text: the row's name stays the speed and its hint, and
  // the adjustable row (not these buttons) is what a screen reader uses.
  return (
    <Pressable
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      accessibilityState={{ disabled }}
      android_ripple={
        isAndroid() ? { color: ripple.neutral, borderless: false } : undefined
      }
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [
        styles.stepButton,
        { width: hit.min, height: hit.min },
        disabled ? styles.stepDisabled : null,
        pressed && !isAndroid() ? styles.stepPressed : null,
      ]}
    >
      <View style={styles.stepBar} />
      {glyph === 'plus' ? (
        <View style={[styles.stepBar, styles.stepBarVertical]} />
      ) : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  section: {
    gap: space.sm,
  },
  modeRow: {
    gap: space.sm,
    paddingHorizontal: 14,
    paddingVertical: 12,
  },
  hint: {
    fontSize: 13,
    lineHeight: 18,
  },
  footnote: { paddingHorizontal: 14 },
  speedRow: {
    minHeight: 64,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingHorizontal: 14,
    paddingVertical: 8,
  },
  speedCopy: {
    flex: 1,
    gap: 2,
  },
  speedTitleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
  },
  speedButtons: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.sm,
  },
  stepButton: {
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: radius.button,
    backgroundColor: tokens.raised,
    overflow: 'hidden',
  },
  stepBar: {
    position: 'absolute',
    width: 14,
    height: 2,
    borderRadius: 1,
    backgroundColor: tokens.text,
  },
  stepBarVertical: { transform: [{ rotate: '90deg' }] },
  stepPressed: { opacity: press.opacity },
  stepDisabled: { opacity: 0.4 },
});

export default SessionDefaultsSection;
