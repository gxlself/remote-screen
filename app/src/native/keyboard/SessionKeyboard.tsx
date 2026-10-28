import { useCallback, useEffect, useRef, useState } from 'react';
import type { JSX, ReactNode } from 'react';
import {
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { usePlatform } from '../platform/PlatformContext';
import { useCore } from '../rscore/CoreContext';
import { sendFeedbackAction } from '../../features/session/actionFeedback';
import type { SessionId } from '../rscore/types';
import { tokens } from '../../theme/tokens';
import { useSetting } from '../../features/settings/store';
import { KeyCapture } from './KeyCapture';
import type { SessionShortcutEvent } from './KeyCapture';
import { KeyBarButton } from './KeyBarButton';
import {
  ArrowTracker,
  KEY_ESCAPE,
  KEY_TAB,
  NAV_KEYS,
  NO_MODS,
  arrowUsage,
  consumeOnce,
  cycle,
  functionKeyUsage,
  hintText,
  isModifierUsage,
  keyPress,
  mergeMods,
  modsMask,
  navKeyUsage,
  pasteKeys,
  shortcut,
  splitUtf8,
} from './keyBar';
import type {
  Arrow,
  InputMode,
  KeyEvent,
  ModKey,
  ModStates,
  ShortcutId,
} from './keyBar';
import {
  KEYBAR_HEIGHT,
  KEYBAR_HINT_HEIGHT,
  resetKeyboardInset,
  setKeyboardInset,
} from './keyboardInset';
import {
  MAC_SHORTCUTS,
  WINDOWS_SHORTCUTS,
  keyboardStrings,
  modifierA11y,
  modifierKeysFor,
  modifierLabel,
  shortcutA11y,
  shortcutLabel,
} from './strings';

export type SessionKeyboardProps = {
  open: boolean;
  sessionId: SessionId | null;
  os: 'macos' | 'windows';
  leading?: ReactNode;
  onClose: () => void;
  /**
   * Pushes this device's clipboard to the computer. Given, and with
   * keyboard.pasteSync on, a paste (hardware ⌘V, the bar's ⌘V / Ctrl+V)
   * waits for it before the keys go out.
   */
  onBeforePaste?: () => Promise<void>;
  /** ⌃⌥⌘ session shortcuts from a hardware keyboard (switcher.ts). */
  onShortcut?: (event: SessionShortcutEvent) => void;
};

type KeyboardFrame = {
  height: number;
  visible: boolean;
  hardware: boolean;
};

type Toast = { message: string };

const PASTE_CONFIRM_CHARS = 2000;
const TOAST_MS = 2000;
const ARROW_KEY_SIZE = 44;
const SEGMENT_OPTION_HEIGHT = 36;
const ARROWS: readonly Arrow[] = ['up', 'down', 'left', 'right'];

/** 轻点方向键时按落点离中心的方向选一个箭头；拿不到落点时返回 null。 */
export function tapArrow(locationX: number, locationY: number): Arrow | null {
  if (!Number.isFinite(locationX) || !Number.isFinite(locationY)) {
    return null;
  }
  const dx = locationX - ARROW_KEY_SIZE / 2;
  const dy = locationY - ARROW_KEY_SIZE / 2;
  if (Math.abs(dx) >= Math.abs(dy)) {
    return dx >= 0 ? 'right' : 'left';
  }
  return dy >= 0 ? 'down' : 'up';
}

function isArrow(name: string): name is Arrow {
  return (ARROWS as readonly string[]).includes(name);
}

function readIsPad(platform: ReturnType<typeof usePlatform>): boolean {
  try {
    return platform.deviceInfo().isPad;
  } catch {
    return false;
  }
}

/**
 * Side padding of the bar. The fixed 59pt in landscape clears an iPhone's
 * notch; an iPad has none, so it follows the real safe area there.
 */
export function keyBarPadding(
  landscape: boolean,
  sideInset: number,
  isPad: boolean,
): number {
  return landscape && !isPad ? Math.max(59, sideInset + 12) : 12 + sideInset;
}

function modeLabel(mode: InputMode): string {
  return keyboardStrings.inputMode[mode];
}

function CmdAsCtrlSwitch({
  checked,
  onToggle,
}: {
  checked: boolean;
  onToggle: () => void;
}): JSX.Element {
  return (
    <Pressable
      accessibilityLabel={keyboardStrings.a11y.cmdAsCtrl}
      accessibilityRole="switch"
      accessibilityState={{ checked }}
      onPress={onToggle}
      style={styles.switchPress}
      testID="keybar-cmd-as-ctrl"
    >
      <View style={[styles.switchPill, checked ? styles.switchPillOn : null]}>
        <View style={[styles.switchBox, checked ? styles.switchBoxOn : null]}>
          {checked ? (
            <Icon color="accentText" name="check" size={10} strokeWidth={2.4} />
          ) : null}
        </View>
        <AppText
          style={[styles.switchGlyph, checked ? styles.switchGlyphOn : null]}
        >
          {keyboardStrings.cmdAsCtrl.glyph}
        </AppText>
        <AppText
          style={[styles.switchLabel, checked ? styles.switchLabelOn : null]}
        >
          {keyboardStrings.cmdAsCtrl.label}
        </AppText>
      </View>
    </Pressable>
  );
}

export function SessionKeyboard({
  open,
  sessionId,
  os,
  leading,
  onClose,
  onBeforePaste,
  onShortcut,
}: SessionKeyboardProps): JSX.Element {
  const core = useCore();
  const platform = usePlatform();
  const [isPad] = useState(() => readIsPad(platform));
  const [cmdAsCtrl, setCmdAsCtrl] = useSetting('keyboard.cmdAsCtrl');
  const [pasteSyncSetting] = useSetting('keyboard.pasteSync');
  const insets = useSafeAreaInsets();
  const { width, height } = useWindowDimensions();
  const [mode, setMode] = useState<InputMode>('text');
  const [state, setState] = useState<ModStates>(NO_MODS);
  const [composition, setComposition] = useState('');
  const [frame, setFrame] = useState<KeyboardFrame>({
    height: 0,
    visible: false,
    hardware: false,
  });
  const [showFunctions, setShowFunctions] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const tracker = useRef<ArrowTracker | null>(null);
  const arrowLast = useRef<{ x: number; y: number } | null>(null);
  const arrowTap = useRef<{ x: number; y: number; moved: boolean } | null>(
    null,
  );
  const [arrowActive, setArrowActive] = useState(false);

  const mods = modsMask(state);
  const hasSession = sessionId !== null;
  // KeyCapture only has two OS modes. Sending "macos" leaves Command as GUI
  // (Win on the host) instead of remapping it to Ctrl.
  const captureOs = os === 'windows' && !cmdAsCtrl ? 'macos' : os;
  // Without the remap a Windows ⌘V is Win+V (clipboard history), not paste.
  const beforePaste =
    pasteSyncSetting && (os === 'macos' || cmdAsCtrl)
      ? onBeforePaste
      : undefined;

  const showToast = useCallback((message: string) => {
    setToast({ message });
    if (toastTimer.current !== null) {
      clearTimeout(toastTimer.current);
    }
    toastTimer.current = setTimeout(() => {
      toastTimer.current = null;
      setToast(null);
    }, TOAST_MS);
  }, []);

  const sendKeys = useCallback(
    (events: KeyEvent[]) => {
      if (sessionId === null) {
        return;
      }
      try {
        for (const event of events) {
          core.sessions.key(sessionId, event.usage, event.down, event.mods);
        }
      } catch {
        core.logs.write('warn', 'keyboard', 'Failed to send key event');
        showToast(keyboardStrings.a11y.sendFailed);
      }
    },
    [core, sessionId, showToast],
  );

  const sendText = useCallback(
    (text: string) => {
      if (sessionId === null) {
        return;
      }
      try {
        for (const part of splitUtf8(text)) {
          core.sessions.text(sessionId, part);
        }
      } catch {
        core.logs.write('warn', 'keyboard', 'Failed to send text event');
        showToast(keyboardStrings.a11y.sendFailed);
      }
    },
    [core, sessionId, showToast],
  );

  const consume = useCallback(() => {
    setState(current => consumeOnce(current));
  }, []);

  const pressKey = useCallback(
    (usage: number) => {
      if (sessionId === null) {
        return;
      }
      sendKeys(keyPress(usage, mods));
      consume();
    },
    [consume, mods, sendKeys, sessionId],
  );

  const afterPasteSync = useCallback(
    (send: () => void) => {
      if (beforePaste === undefined) {
        send();
        return;
      }
      // A failed push still pastes what the computer already has.
      beforePaste().then(send, send);
    },
    [beforePaste],
  );

  const pasteFromHardware = useCallback(
    (held: number) => {
      afterPasteSync(() => sendKeys(pasteKeys(captureOs, held)));
    },
    [afterPasteSync, captureOs, sendKeys],
  );

  const runShortcut = useCallback(
    (id: ShortcutId) => {
      if (sessionId === null) {
        return;
      }
      const plan = shortcut(id, os);
      if (plan.kind === 'keys') {
        const target = plan.events.find(
          event => event.down && !isModifierUsage(event.usage),
        );
        const events =
          target === undefined
            ? plan.events
            : keyPress(target.usage, mergeMods(mods, target.mods));
        consume();
        if (id === 'cmdV' || id === 'ctrlV') {
          afterPasteSync(() => sendKeys(events));
        } else {
          sendKeys(events);
        }
      } else if (plan.kind === 'combo') {
        try {
          core.sessions.combo(sessionId, plan.combo);
        } catch {
          core.logs.write('warn', 'keyboard', 'Failed to send shortcut');
          showToast(keyboardStrings.a11y.sendFailed);
        }
      } else if (plan.kind === 'action') {
        // 和工具栏、「更多」走同一条路，结果由 ActionFeedback 提示。
        if (!sendFeedbackAction(core, sessionId, plan.action)) {
          core.logs.write('warn', 'keyboard', 'Failed to send shortcut');
          showToast(keyboardStrings.a11y.sendFailed);
        }
      }
    },
    [afterPasteSync, consume, core, mods, os, sendKeys, sessionId, showToast],
  );

  const pasteAsTyping = useCallback(async () => {
    if (sessionId === null) {
      return;
    }
    let text: string | null;
    try {
      text = await platform.readText();
    } catch {
      showToast(keyboardStrings.a11y.pasteReadFailed);
      return;
    }
    if (text === null || text.length === 0) {
      showToast(keyboardStrings.a11y.clipboardEmpty);
      return;
    }

    const count = Array.from(text).length;
    if (count <= PASTE_CONFIRM_CHARS) {
      sendText(text);
      return;
    }

    Alert.alert(
      keyboardStrings.a11y.pasteConfirmTitle,
      keyboardStrings.a11y.pasteConfirm(count.toLocaleString('en-US')),
      [
        { text: keyboardStrings.a11y.cancel, style: 'cancel' },
        {
          text: keyboardStrings.a11y.confirm,
          onPress: () => sendText(text),
        },
      ],
    );
  }, [platform, sendText, sessionId, showToast]);

  useEffect(
    () => () => {
      if (toastTimer.current !== null) {
        clearTimeout(toastTimer.current);
      }
    },
    [],
  );

  useEffect(() => {
    if (!open) {
      resetKeyboardInset();
      return;
    }
    const hardwareAtBottom = frame.hardware && frame.height <= insets.bottom;
    const overlap =
      frame.visible && !hardwareAtBottom ? Math.max(0, frame.height) : 0;
    // Measure to the top of the hint row, wherever the anchor puts the bar,
    // so toolbars lifted by this inset never overlap it.
    const barBottom = overlap > 0 ? overlap : insets.bottom;
    setKeyboardInset(barBottom + KEYBAR_HEIGHT + KEYBAR_HINT_HEIGHT);
  }, [frame, insets.bottom, open]);

  useEffect(() => {
    return resetKeyboardInset;
  }, []);

  const onKeyboardFrame = useCallback((next: KeyboardFrame) => {
    setFrame(next);
  }, []);

  const startArrow = (
    pageX: number,
    pageY: number,
    locationX: number,
    locationY: number,
  ) => {
    tracker.current = new ArrowTracker();
    tracker.current.start();
    arrowLast.current = { x: pageX, y: pageY };
    arrowTap.current = { x: locationX, y: locationY, moved: false };
    setArrowActive(true);
  };

  const moveArrow = (pageX: number, pageY: number) => {
    const last = arrowLast.current;
    const activeTracker = tracker.current;
    if (last === null || activeTracker === null) {
      return;
    }
    const dx = pageX - last.x;
    const dy = pageY - last.y;
    arrowLast.current = { x: pageX, y: pageY };
    for (const arrow of activeTracker.move(dx, dy)) {
      if (arrowTap.current !== null) {
        arrowTap.current.moved = true;
      }
      pressKey(arrowUsage(arrow));
    }
  };

  const endArrow = () => {
    const tap = arrowTap.current;
    tracker.current = null;
    arrowLast.current = null;
    arrowTap.current = null;
    setArrowActive(false);
    // 轻点没拖动：按点在键的哪一边发一次方向键。
    const arrow = tap === null || tap.moved ? null : tapArrow(tap.x, tap.y);
    if (arrow !== null) {
      pressKey(arrowUsage(arrow));
    }
  };

  const safeBottom = insets.bottom;
  const hardwareAtBottom = frame.hardware && frame.height <= safeBottom;
  const keyboardOverlap =
    frame.visible && !hardwareAtBottom ? Math.max(0, frame.height) : 0;
  const bottom = keyboardOverlap > 0 ? keyboardOverlap : safeBottom;
  const sideInset = Math.max(insets.left, insets.right);
  const landscape = width > height;
  const padding = keyBarPadding(landscape, sideInset, isPad);
  const shortcuts = os === 'macos' ? MAC_SHORTCUTS : WINDOWS_SHORTCUTS;

  return (
    <View pointerEvents="box-none" style={StyleSheet.absoluteFill}>
      <KeyCapture
        active={open}
        inputMode={mode}
        onActiveChange={active => {
          if (!active) {
            onClose();
          }
        }}
        onComposition={setComposition}
        onKeyboardFrame={onKeyboardFrame}
        onPaste={pasteFromHardware}
        onShortcut={onShortcut}
        onStickyConsumed={consume}
        os={captureOs}
        sessionId={sessionId}
        stickyMods={mods}
        syncPaste={beforePaste !== undefined}
      />

      {open ? (
        <View
          pointerEvents="box-none"
          style={[styles.anchor, { bottom }]}
          testID="session-keyboard"
        >
          <View
            style={[styles.hintRow, { paddingHorizontal: padding }]}
            testID="keybar-hint"
          >
            {leading == null ? null : (
              <View style={styles.leadingSlot}>{leading}</View>
            )}
            {os === 'windows' ? (
              <CmdAsCtrlSwitch
                checked={cmdAsCtrl}
                onToggle={() => setCmdAsCtrl(!cmdAsCtrl)}
              />
            ) : null}
            <View style={styles.hintSpacer} />
            <View
              accessibilityLabel={keyboardStrings.a11y.hintBar}
              accessibilityLiveRegion="polite"
              accessibilityRole="status"
              accessible
              style={styles.hintPill}
              testID="keybar-hint-pill"
            >
              <AppText style={styles.hintText} variant="caption">
                {hintText(mode, state, os)}
              </AppText>
              {composition === '' ? null : (
                <AppText
                  color="text3"
                  numberOfLines={1}
                  style={styles.composition}
                  variant="caption"
                >
                  {keyboardStrings.a11y.composition(composition)}
                </AppText>
              )}
            </View>
          </View>

          {toast === null ? null : (
            <View style={styles.toastWrap}>
              <View
                accessibilityLiveRegion="polite"
                accessible
                style={styles.toast}
                testID="keybar-toast"
              >
                <AppText variant="caption">{toast.message}</AppText>
              </View>
            </View>
          )}

          <View
            accessibilityRole="toolbar"
            style={[styles.bar, { paddingHorizontal: padding }]}
            testID="keybar"
          >
            <ScrollView
              contentContainerStyle={styles.row}
              horizontal
              scrollEnabled={!arrowActive}
              showsHorizontalScrollIndicator={false}
              style={styles.scroll}
              testID={showFunctions ? 'keybar-functions' : 'keybar-main'}
            >
              {showFunctions ? (
                <>
                  <KeyBarButton
                    accessibilityLabel={keyboardStrings.keys.back}
                    label={keyboardStrings.keys.back}
                    onPress={() => setShowFunctions(false)}
                    testID="keybar-functions-back"
                  />
                  {Array.from({ length: 12 }, (_, index) => index + 1).map(
                    number => (
                      <KeyBarButton
                        accessibilityLabel={keyboardStrings.a11y.functionKey(
                          number,
                        )}
                        disabled={!hasSession}
                        key={number}
                        label={`F${number}`}
                        onPress={() => pressKey(functionKeyUsage(number))}
                        testID={`keybar-f${number}`}
                      />
                    ),
                  )}
                </>
              ) : (
                <>
                  <View
                    accessibilityLabel={keyboardStrings.inputModeLabel}
                    accessibilityRole="radiogroup"
                    style={styles.segment}
                    testID="keybar-mode"
                  >
                    {(['text', 'keys'] as const).map(value => (
                      <KeyBarButton
                        accessibilityLabel={
                          keyboardStrings.inputModeA11y[value]
                        }
                        key={value}
                        label={modeLabel(value)}
                        onPress={() => setMode(value)}
                        height={SEGMENT_OPTION_HEIGHT}
                        selected={mode === value}
                        style={styles.segmentOption}
                        testID={`keybar-mode-${value}`}
                        variant="segment"
                      />
                    ))}
                  </View>

                  {modifierKeysFor(os).map((key: ModKey) => (
                    <KeyBarButton
                      accessibilityLabel={modifierA11y(os, key, state[key])}
                      key={key}
                      label={modifierLabel(os, key)}
                      modifierState={state[key]}
                      onPress={() =>
                        setState(current => ({
                          ...current,
                          [key]: cycle(current[key]),
                        }))
                      }
                      testID={`keybar-mod-${key}`}
                      variant="modifier"
                    />
                  ))}

                  <KeyBarButton
                    accessibilityLabel={keyboardStrings.keys.escape}
                    disabled={!hasSession}
                    label={keyboardStrings.keys.escape}
                    onPress={() => pressKey(KEY_ESCAPE)}
                    testID="keybar-esc"
                  />
                  <KeyBarButton
                    accessibilityLabel={keyboardStrings.keys.tab}
                    disabled={!hasSession}
                    label={keyboardStrings.keys.tab}
                    onPress={() => pressKey(KEY_TAB)}
                    testID="keybar-tab"
                  />
                  <KeyBarButton
                    accessibilityActions={ARROWS.map(arrow => ({
                      name: arrow,
                      label: keyboardStrings.a11y.arrows[arrow],
                    }))}
                    accessibilityLabel={keyboardStrings.a11y.direction}
                    cancelable={false}
                    disabled={!hasSession}
                    label={<Icon color="text" name="dpad" size={18} />}
                    onAccessibilityAction={event => {
                      const name = event.nativeEvent.actionName;
                      if (isArrow(name)) {
                        pressKey(arrowUsage(name));
                      }
                    }}
                    onPressIn={startArrow}
                    onPressMove={moveArrow}
                    onPressOut={endArrow}
                    testID="keybar-arrows"
                  />
                  {NAV_KEYS.map(key => (
                    <KeyBarButton
                      accessibilityLabel={keyboardStrings.a11y.navKeys[key]}
                      disabled={!hasSession}
                      key={key}
                      label={keyboardStrings.keys[key]}
                      onPress={() => pressKey(navKeyUsage(key))}
                      testID={`keybar-${key}`}
                    />
                  ))}
                  <KeyBarButton
                    accessibilityLabel={keyboardStrings.a11y.functionKeys}
                    label={keyboardStrings.keys.functionKeys}
                    onPress={() => setShowFunctions(true)}
                    testID="keybar-functions-open"
                  />

                  {shortcuts.map((id: ShortcutId) => (
                    <KeyBarButton
                      accessibilityLabel={shortcutA11y(id)}
                      disabled={!hasSession}
                      key={id}
                      label={shortcutLabel(id)}
                      onPress={() => {
                        if (id === 'pasteAsTyping') {
                          pasteAsTyping().catch(() => {
                            core.logs.write(
                              'warn',
                              'keyboard',
                              'Failed to read clipboard',
                            );
                          });
                        } else {
                          runShortcut(id);
                        }
                      }}
                      testID={`keybar-shortcut-${id}`}
                    />
                  ))}
                </>
              )}
            </ScrollView>
            <View
              accessibilityElementsHidden
              importantForAccessibility="no-hide-descendants"
              style={styles.divider}
            />
            <KeyBarButton
              accessibilityLabel={keyboardStrings.keys.close}
              label={<Icon color="text" name="keyboardHide" size={18} />}
              onPress={onClose}
              testID="keybar-close"
              variant="plain"
            />
          </View>
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  anchor: { position: 'absolute', left: 0, right: 0 },
  hintRow: {
    height: KEYBAR_HINT_HEIGHT,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    backgroundColor: tokens.bg,
  },
  leadingSlot: { flexDirection: 'row', alignItems: 'center' },
  hintSpacer: { flex: 1 },
  hintPill: {
    maxWidth: 340,
    minHeight: 30,
    flexShrink: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 15,
    backgroundColor: 'rgba(10,13,16,0.8)',
  },
  hintText: { flexShrink: 1, color: '#D5DCE0', fontSize: 12 },
  composition: { flexShrink: 1 },
  switchPress: { height: 44, justifyContent: 'center' },
  switchPill: {
    height: 30,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 7,
    paddingLeft: 8,
    paddingRight: 11,
    borderRadius: 15,
    backgroundColor: 'rgba(10,13,16,0.8)',
  },
  switchPillOn: {
    backgroundColor: 'rgba(125,168,255,0.16)',
    borderWidth: 1,
    borderColor: 'rgba(125,168,255,0.6)',
  },
  switchBox: {
    width: 15,
    height: 15,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 4,
    borderWidth: 1.5,
    borderColor: '#5B6670',
  },
  switchBoxOn: {
    borderWidth: 0,
    backgroundColor: tokens.accent,
  },
  switchGlyph: { color: '#A9B4BB', fontSize: 13, lineHeight: 16 },
  switchGlyphOn: { color: tokens.text },
  switchLabel: { color: '#A9B4BB', fontSize: 12, lineHeight: 16 },
  switchLabelOn: { color: tokens.text },
  toastWrap: { alignItems: 'center', paddingBottom: 6 },
  toast: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 999,
    backgroundColor: tokens.raised,
  },
  bar: {
    height: KEYBAR_HEIGHT,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    borderTopWidth: 1,
    borderTopColor: tokens.panelLine,
    backgroundColor: '#161B1F',
  },
  scroll: { flex: 1 },
  row: { alignItems: 'center', gap: 6 },
  divider: { width: 1, height: 24, backgroundColor: tokens.inputLine },
  segment: {
    height: 40,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    padding: 2,
    borderRadius: 10,
    backgroundColor: tokens.raised,
  },
  segmentOption: { height: SEGMENT_OPTION_HEIGHT, minWidth: 52 },
});

export default SessionKeyboard;
