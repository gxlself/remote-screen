import { useRef, useState } from 'react';
import type { JSX } from 'react';
import {
  Animated,
  PanResponder,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../session/WindowContext';
import type { AccessibilityActionEvent, ViewStyle } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { IconButton } from '../../components/IconButton';
import { adapters } from '../../i18n/zh/adapters';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import { switcher } from '../../i18n/zh/switcher';
import type {
  AdapterProtocol,
  AdapterScreen,
  SessionCombo,
} from '../../native/rscore/types';
import { radius, tokens } from '../../theme/tokens';
import { AdapterShortcutKeys } from './AdapterShortcutKeys';
import { dockForPoint, TOOLBAR_DOCKS, type ToolbarDock } from './logic';
import type { MonitorCount } from './monitorMemory';
import { ScreenPicker } from './ScreenPicker';

/** Distance from the screen edge (or the keyboard bar) to the toolbar. */
const TOOLBAR_GAP = 12;
/** Movement before a press on the grip turns into a drag. */
const DRAG_SLOP = 8;

const MOVE_ACTIONS: Record<ToolbarDock, string> = {
  left: 'moveLeft',
  right: 'moveRight',
  bottom: 'moveBottom',
};

export type AdapterToolbarProps = {
  protocol: AdapterProtocol;
  /** VNC 且这台电脑已保存密码（默认 8）。 */
  canTypePassword: boolean;
  keyboardOpen: boolean;
  /** 键盘打开时按键栏顶部到屏幕底的距离，关闭时为 0。 */
  keyboardInset: number;
  mode: 'trackpad' | 'touch';
  /** 多于一台显示器时才传（C13.6）。 */
  screens?: AdapterScreen[];
  /** 当前显示的那台（screens 的下标），或 WHOLE_DESKTOP。 */
  selectedScreen: number;
  /**
   * 远程桌面才传：显示器按钮一直在，弹出里能改数量。
   */
  monitors?: {
    count: MonitorCount;
    onChange(count: MonitorCount): void;
  };
  /** 收成边上的小把手。 */
  collapsed: boolean;
  /** 贴在哪条边；按设备记住（toolbarDock.ts）。 */
  dock: ToolbarDock;
  onCollapsedChange(collapsed: boolean): void;
  onDock(dock: ToolbarDock): void;
  onSelectScreen(index: number): void;
  onKeyboard(): void;
  onSendClipboard(): void;
  onTypePassword(): void;
  onToggleMode(): void;
  onMore(): void;
  /** V33: leaves the page with the connection kept. */
  onMinimize(): void;
  /** opens the session switcher. */
  onSwitch(): void;
  onDisconnect(): void;
  /** V39: a long press names the button (the screen shows it as a toast). */
  onHint(label: string): void;
  /** V41: iPad gets the shortcuts iPadOS would otherwise take. */
  shortcuts?: boolean;
  onCombo?(combo: SessionCombo): void;
};

/** Drags the toolbar by its grip and docks it where it is dropped. */
function useDockDrag(onDock: (dock: ToolbarDock) => void) {
  const { width } = useWindowSize();
  const latest = useRef({ onDock, width });
  latest.current = { onDock, width };
  const offset = useRef(new Animated.ValueXY()).current;
  const responder = useRef(
    PanResponder.create({
      onMoveShouldSetPanResponderCapture: (_event, gesture) =>
        Math.abs(gesture.dx) > DRAG_SLOP || Math.abs(gesture.dy) > DRAG_SLOP,
      onPanResponderMove: Animated.event(
        [null, { dx: offset.x, dy: offset.y }],
        {
          useNativeDriver: false,
        },
      ),
      onPanResponderRelease: (_event, gesture) => {
        offset.setValue({ x: 0, y: 0 });
        latest.current.onDock(
          dockForPoint(gesture.moveX, latest.current.width),
        );
      },
      onPanResponderTerminate: () => offset.setValue({ x: 0, y: 0 }),
    }),
  ).current;
  return { offset, panHandlers: responder.panHandlers };
}

function moveActions(dock: ToolbarDock) {
  return TOOLBAR_DOCKS.filter(other => other !== dock).map(other => ({
    name: MOVE_ACTIONS[other],
    label: adapters.toolbar.move[other],
  }));
}

function dockFromAction(event: AccessibilityActionEvent): ToolbarDock | null {
  return (
    TOOLBAR_DOCKS.find(
      dock => MOVE_ACTIONS[dock] === event.nativeEvent.actionName,
    ) ?? null
  );
}

export function AdapterToolbar(p: AdapterToolbarProps): JSX.Element {
  const [pickerOpen, setPickerOpen] = useState(false);
  const insets = useSafeAreaInsets();
  const drag = useDockDrag(p.onDock);
  const screens =
    p.screens !== undefined && p.screens.length > 1 ? p.screens : undefined;
  const monitors = p.monitors;
  const screensButton = screens !== undefined || monitors !== undefined;
  if (pickerOpen && !screensButton) {
    setPickerOpen(false);
  }

  const vertical = p.dock !== 'bottom';
  // The keyboard bar already covers the home indicator.
  const bottom =
    p.keyboardInset > 0
      ? p.keyboardInset + TOOLBAR_GAP
      : Math.max(TOOLBAR_GAP, insets.bottom);
  const anchor: ViewStyle =
    p.dock === 'bottom'
      ? {
          left: insets.left + TOOLBAR_GAP,
          right: insets.right + TOOLBAR_GAP,
          bottom,
          alignItems: 'center',
        }
      : {
          top: insets.top + TOOLBAR_GAP,
          bottom,
          justifyContent: 'center',
          ...(p.dock === 'left'
            ? { left: Math.max(TOOLBAR_GAP, insets.left) }
            : { right: Math.max(TOOLBAR_GAP, insets.right) }),
        };
  const onMoveAction = (event: AccessibilityActionEvent) => {
    const dock = dockFromAction(event);
    if (dock !== null) {
      p.onDock(dock);
    }
  };

  if (p.collapsed) {
    return (
      <View
        pointerEvents="box-none"
        style={[styles.anchor, anchor]}
        testID="adapter-toolbar"
      >
        <Animated.View
          {...drag.panHandlers}
          style={{ transform: drag.offset.getTranslateTransform() }}
        >
          <Pressable
            accessibilityActions={moveActions(p.dock)}
            accessibilityHint={adapters.toolbar.dragHint}
            accessibilityLabel={adapters.toolbar.expand}
            accessibilityRole="button"
            hitSlop={8}
            onAccessibilityAction={onMoveAction}
            onPress={() => p.onCollapsedChange(false)}
            style={({ pressed }) => [
              styles.handle,
              vertical ? styles.handleVertical : styles.handleHorizontal,
              pressed ? styles.pressed : null,
            ]}
            testID="adapter-tb-expand"
          >
            <View
              style={[
                styles.handleMark,
                vertical ? styles.markVertical : styles.markHorizontal,
              ]}
            />
          </Pressable>
        </Animated.View>
      </View>
    );
  }

  const keyboardLabel = p.keyboardOpen
    ? adapters.toolbar.keyboardHide
    : adapters.toolbar.keyboard;
  const modeLabel = adapters.toolbar.mode(p.mode);
  const items = (
    <>
      <IconButton
        accessibilityLabel={sessionToolbar.collapse}
        icon="chevronDown"
        onLongPress={() => p.onHint(sessionToolbar.collapse)}
        onPress={p.onMinimize}
        testID="adapter-tb-minimize"
        variant="plain"
      />
      <IconButton
        accessibilityLabel={switcher.a11y.button}
        icon="sessions"
        onLongPress={() => p.onHint(switcher.a11y.button)}
        onPress={p.onSwitch}
        testID="adapter-tb-switch"
        variant="plain"
      />
      <IconButton
        accessibilityLabel={keyboardLabel}
        icon={p.keyboardOpen ? 'keyboardHide' : 'keyboard'}
        onLongPress={() => p.onHint(keyboardLabel)}
        onPress={p.onKeyboard}
        testID="adapter-tb-keyboard"
        variant={p.keyboardOpen ? 'accentSoft' : 'plain'}
      />
      <IconButton
        accessibilityLabel={modeLabel}
        icon={p.mode === 'trackpad' ? 'pointer' : 'hand'}
        onLongPress={() => p.onHint(modeLabel)}
        onPress={p.onToggleMode}
        testID="adapter-tb-mode"
        variant="plain"
      />
      <IconButton
        accessibilityLabel={adapters.toolbar.sendClipboard}
        icon="clipboard"
        onLongPress={() => p.onHint(adapters.toolbar.sendClipboard)}
        onPress={p.onSendClipboard}
        testID="adapter-tb-sendClipboard"
        variant="plain"
      />
      {p.canTypePassword ? (
        <IconButton
          accessibilityLabel={adapters.toolbar.typePassword}
          icon="key"
          onLongPress={() => p.onHint(adapters.toolbar.typePassword)}
          onPress={p.onTypePassword}
          testID="adapter-tb-typePassword"
          variant="plain"
        />
      ) : null}
      {!screensButton ? null : (
        <IconButton
          accessibilityLabel={adapters.a11y.screens(
            screens?.length ?? monitors?.count ?? 1,
          )}
          icon="displays"
          onLongPress={() => p.onHint(adapters.toolbar.screens)}
          onPress={() => setPickerOpen(true)}
          testID="adapter-tb-screens"
          variant={pickerOpen ? 'accentSoft' : 'plain'}
        />
      )}
      {p.shortcuts && p.onCombo !== undefined ? (
        <AdapterShortcutKeys
          onCombo={p.onCombo}
          onHint={p.onHint}
          os={p.protocol === 'rdp' ? 'windows' : 'macos'}
          vertical={vertical}
        />
      ) : null}
      <IconButton
        accessibilityLabel={adapters.toolbar.more}
        icon="more"
        onLongPress={() => p.onHint(adapters.toolbar.more)}
        onPress={p.onMore}
        testID="adapter-tb-more"
        variant="plain"
      />
    </>
  );

  return (
    <View
      accessibilityLabel={adapters.a11y.toolbar}
      accessibilityRole="toolbar"
      pointerEvents="box-none"
      style={[styles.anchor, anchor]}
      testID="adapter-toolbar"
    >
      <Animated.View
        style={[
          styles.bar,
          vertical ? styles.barVertical : styles.barHorizontal,
          { transform: drag.offset.getTranslateTransform() },
        ]}
        testID="adapter-toolbar-bar"
      >
        <View {...drag.panHandlers}>
          <Pressable
            accessibilityActions={moveActions(p.dock)}
            accessibilityHint={adapters.toolbar.dragHint}
            accessibilityLabel={adapters.toolbar.collapse}
            accessibilityRole="button"
            onAccessibilityAction={onMoveAction}
            onPress={() => p.onCollapsedChange(true)}
            style={({ pressed }) => [
              styles.grip,
              pressed ? styles.pressed : null,
            ]}
            testID="adapter-tb-collapse"
          >
            <View
              style={[
                styles.handleMark,
                vertical ? styles.markHorizontal : styles.markVertical,
              ]}
            />
          </Pressable>
        </View>
        <ScrollView
          alwaysBounceHorizontal={false}
          alwaysBounceVertical={false}
          contentContainerStyle={[
            styles.items,
            vertical ? styles.itemsVertical : styles.itemsHorizontal,
          ]}
          horizontal={!vertical}
          showsHorizontalScrollIndicator={false}
          showsVerticalScrollIndicator={false}
          style={styles.scroll}
        >
          {items}
        </ScrollView>
        <View
          accessibilityElementsHidden
          importantForAccessibility="no-hide-descendants"
          style={vertical ? styles.dividerVertical : styles.divider}
        />
        <IconButton
          accessibilityLabel={adapters.toolbar.disconnect}
          icon="close"
          onLongPress={() => p.onHint(adapters.toolbar.disconnect)}
          onPress={p.onDisconnect}
          testID="adapter-tb-disconnect"
          variant="raised"
        />
      </Animated.View>
      {pickerOpen && screensButton ? (
        <ScreenPicker
          onClose={() => setPickerOpen(false)}
          onSelect={index => {
            setPickerOpen(false);
            p.onSelectScreen(index);
          }}
          screens={screens ?? []}
          selected={p.selectedScreen}
          {...(monitors === undefined
            ? {}
            : {
                monitors: {
                  count: monitors.count,
                  onChange: (count: MonitorCount) => {
                    setPickerOpen(false);
                    monitors.onChange(count);
                  },
                },
              })}
        />
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  anchor: {
    position: 'absolute',
  },
  bar: {
    alignItems: 'center',
    gap: 4,
    padding: 4,
    borderRadius: radius.toolbar,
    borderWidth: 1,
    borderColor: tokens.hudLine,
    backgroundColor: tokens.toolbar,
  },
  barHorizontal: { flexDirection: 'row', maxWidth: '100%' },
  barVertical: { flexDirection: 'column', maxHeight: '100%' },
  scroll: { flexShrink: 1, flexGrow: 0 },
  items: { alignItems: 'center', gap: 4 },
  itemsHorizontal: { flexDirection: 'row' },
  itemsVertical: { flexDirection: 'column' },
  grip: {
    width: 28,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 14,
  },
  handle: {
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: radius.toolbar,
    borderWidth: 1,
    borderColor: tokens.hudLine,
    backgroundColor: tokens.toolbar,
  },
  handleHorizontal: { width: 64, height: 28 },
  handleVertical: { width: 28, height: 64 },
  handleMark: {
    borderRadius: 2,
    backgroundColor: tokens.text3,
  },
  markHorizontal: { width: 28, height: 4 },
  markVertical: { width: 4, height: 20 },
  pressed: { opacity: 0.6 },
  divider: {
    width: 1,
    height: 24,
    marginHorizontal: 4,
    backgroundColor: tokens.inputLine,
  },
  dividerVertical: {
    width: 24,
    height: 1,
    marginVertical: 4,
    backgroundColor: tokens.inputLine,
  },
});

export default AdapterToolbar;
