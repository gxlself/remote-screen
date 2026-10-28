import type { JSX, ReactNode } from 'react';
import {
  PixelRatio,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import {
  NARROW_WINDOW,
  useWindowSize,
} from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import type { IconName } from '../../components/icons/Icon';
import { IconButton } from '../../components/IconButton';
import { Segmented } from '../../components/Segmented';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import { switcher } from '../../i18n/zh/switcher';
import { sendFeedbackAction } from '../../features/session/actionFeedback';
import { useKeyboardInset } from '../../native/keyboard/keyboardInset';
import { useCore } from '../../native/rscore/CoreContext';
import { hit, radius, tokens } from '../../theme/tokens';
import { IpadShortcutKeys } from './IpadShortcutKeys';
import type { SlotProps } from './slots';

const NARROW_WIDTH = 700;
const TOOLBAR_BOTTOM = 12;

function Divider() {
  return (
    <View
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      style={styles.divider}
    />
  );
}

function LabelControl({
  icon,
  label,
  mono = false,
  accessibilityLabel,
  accessibilityHint,
  onPress,
  paddingHorizontal,
  testID,
}: {
  icon: IconName;
  label: ReactNode;
  mono?: boolean;
  accessibilityLabel: string;
  accessibilityHint?: string;
  onPress: () => void;
  paddingHorizontal: number;
  testID: string;
}) {
  const hitPadding = Math.max(0, (hit.min - 44) / 2);
  return (
    <Pressable
      accessibilityHint={accessibilityHint}
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="button"
      hitSlop={{ top: hitPadding, bottom: hitPadding }}
      onPress={onPress}
      style={({ pressed }) => [
        styles.labelControl,
        PixelRatio.getFontScale() > 2
          ? { height: undefined, minHeight: 44 }
          : null,
        { paddingHorizontal },
        pressed ? styles.pressed : null,
      ]}
      testID={testID}
    >
      <Icon color="text" name={icon} size={18} />
      {mono ? (
        <AppText mono color="text" variant="mono">
          {label}
        </AppText>
      ) : (
        <AppText color="text" variant="footnote" weight={500}>
          {label}
        </AppText>
      )}
    </Pressable>
  );
}

function CtrlAltDelButton({ onPress }: { onPress: () => void }) {
  const hitPadding = Math.max(0, (hit.min - 44) / 2);
  return (
    <Pressable
      accessibilityLabel={sessionToolbar.ctrlAltDelA11y}
      accessibilityRole="button"
      hitSlop={{ top: hitPadding, bottom: hitPadding }}
      onPress={onPress}
      style={({ pressed }) => [
        styles.ctrlAltDel,
        PixelRatio.getFontScale() > 2
          ? { height: undefined, minHeight: 44 }
          : null,
        pressed ? styles.pressed : null,
      ]}
      testID="toolbar-ctrl-alt-del"
    >
      <AppText mono color="text" variant="monoSmall">
        {sessionToolbar.ctrlAltDel}
      </AppText>
    </Pressable>
  );
}

export function SessionToolbar({ ctx }: SlotProps): JSX.Element | null {
  const core = useCore();
  const { width } = useWindowSize();
  const keyboardInset = useKeyboardInset();
  const { view } = ctx;
  const showClipboard = width >= NARROW_WIDTH;
  const largeText = PixelRatio.getFontScale() > 2;

  const sendCtrlAltDel = () => {
    if (ctx.sessionId === null) {
      return;
    }
    // 结果由 ActionFeedback 提示：失败报原因，没失败才说「已发送」。
    if (!sendFeedbackAction(core, ctx.sessionId, 'ctrlAltDel')) {
      ctx.showToast(sessionToolbar.ctrlAltDelFailed, { tone: 'bad' });
    }
  };

  const controls = (
    <>
      <IconButton
        accessibilityLabel={sessionToolbar.a11y.collapse}
        icon="chevronDown"
        onPress={ctx.collapse}
        testID="toolbar-collapse"
        variant="plain"
      />
      <IconButton
        accessibilityLabel={switcher.a11y.button}
        icon="sessions"
        onPress={() => ctx.openSheet('switcher')}
        testID="toolbar-switch"
        variant="plain"
      />
      <IconButton
        accessibilityLabel={
          view.toolbar.keyboardEnabled
            ? ctx.keyboardOpen
              ? sessionToolbar.a11y.keyboard.hide
              : sessionToolbar.a11y.keyboard.open
            : sessionToolbar.a11y.keyboard.disabled
        }
        disabled={!view.toolbar.keyboardEnabled}
        icon={ctx.keyboardOpen ? 'keyboardHide' : 'keyboard'}
        onPress={ctx.toggleKeyboard}
        testID="toolbar-keyboard"
      />
      {/* a narrow split-view window gets the phone toolbar. */}
      {ctx.isPad && width >= NARROW_WINDOW ? (
        <IpadShortcutKeys ctx={ctx} />
      ) : null}
      <Divider />
      {view.toolbar.showCtrlAltDel ? (
        <CtrlAltDelButton onPress={sendCtrlAltDel} />
      ) : null}
      {view.toolbar.showModeSwitch ? (
        <Segmented
          accessibilityLabel={sessionToolbar.modeLabel}
          onChange={ctx.setMode}
          options={[
            { value: 'trackpad', label: sessionToolbar.mode.trackpad },
            { value: 'touch', label: sessionToolbar.mode.touch },
          ]}
          testID="toolbar-mode"
          value={ctx.mode}
          variant="overlay"
        />
      ) : null}
      {view.toolbar.displayA11y === null ? null : view.toolbar.displayLabel ===
        null ? (
        <IconButton
          accessibilityLabel={sessionToolbar.a11y.displays}
          icon="displays"
          onPress={() => ctx.openSheet('displays')}
          testID="toolbar-displays"
        />
      ) : (
        <LabelControl
          accessibilityLabel={view.toolbar.displayA11y}
          icon="displays"
          label={view.toolbar.displayLabel}
          mono
          onPress={() => ctx.openSheet('displays')}
          paddingHorizontal={10}
          testID="toolbar-displays"
        />
      )}
      <LabelControl
        accessibilityHint={ctx.hud.a11y}
        accessibilityLabel={view.toolbar.qualityA11y}
        icon="quality"
        label={view.toolbar.qualityLabel}
        onPress={() => ctx.openSheet('quality')}
        paddingHorizontal={10}
        testID="toolbar-quality"
      />
      {showClipboard ? (
        <IconButton
          accessibilityLabel={sessionToolbar.a11y.clipboard}
          icon="clipboard"
          onPress={() => ctx.openSheet('clipboard')}
          testID="toolbar-clipboard"
        />
      ) : null}
      <IconButton
        accessibilityLabel={sessionToolbar.a11y.more}
        icon="more"
        onPress={() => ctx.openSheet('more')}
        testID="toolbar-more"
      />
      <Divider />
      <Button
        accessibilityLabel={sessionToolbar.disconnect}
        onPress={ctx.disconnect}
        testID="toolbar-disconnect"
        title={sessionToolbar.disconnect}
        variant="pillDanger"
      />
    </>
  );

  return (
    <View
      accessibilityLabel={sessionToolbar.toolbarLabel}
      accessibilityRole="toolbar"
      pointerEvents="box-none"
      style={[
        styles.anchor,
        { bottom: TOOLBAR_BOTTOM + (ctx.keyboardOpen ? keyboardInset : 0) },
      ]}
    >
      {largeText ? (
        <ScrollView
          contentContainerStyle={styles.scrollContent}
          horizontal
          style={[styles.bar, { maxWidth: width - 2 * hit.landscapeSafe }]}
        >
          {controls}
        </ScrollView>
      ) : (
        <View style={styles.bar}>{controls}</View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  anchor: {
    position: 'absolute',
    left: 0,
    right: 0,
    alignItems: 'center',
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'nowrap',
    gap: 4,
    padding: 4,
    borderRadius: radius.toolbar,
    borderWidth: 1,
    borderColor: tokens.hudLine,
    backgroundColor: tokens.toolbar,
  },
  scrollContent: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    padding: 4,
  },
  divider: {
    width: 1,
    height: 24,
    marginHorizontal: 4,
    backgroundColor: tokens.inputLine,
  },
  labelControl: {
    height: 44,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    borderRadius: 22,
  },
  ctrlAltDel: {
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 12,
    borderRadius: 22,
  },
  pressed: {
    opacity: 0.8,
  },
});

export default SessionToolbar;
