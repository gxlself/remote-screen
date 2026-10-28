import {
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import type React from 'react';
import {
  PanResponder,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import type { PanResponderInstance, ViewInstance } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { hit, tokens } from '../../theme/tokens';
import type { SshId } from '../rscore/types';
import { TerminalView } from './TerminalView';
import type { TerminalHandle, TerminalViewHandle } from './TerminalView';
import { TermKeyButton } from './TermKeyButton';
import {
  SwipeArrows,
  consumeOnce,
  cycle,
  modA11y,
  modHint,
  rowKeys,
  termMods,
} from './termKeys';
import type { ModState, RowKey, SwipeDirection, TermMods } from './termKeys';
import { terminalBarStrings } from './strings';
import { resetTerminalKeyboard, setTerminalKeyboard } from './terminalKeyboard';

export type TerminalPaneProps = {
  sshId: SshId | null;
  fontSize: number;
  theme: 'night';
  accessibilityLabel: string;
  active?: boolean;
  accessory?: React.ReactNode;
  onCopied?: () => void;
  ref?: React.Ref<TerminalHandle>;
};

type KeyboardFrame = {
  height: number;
  visible: boolean;
  hardware: boolean;
};

const NO_MODS: TermMods = { ctrl: 'none', alt: 'none' };
const SWIPE_TAKEOVER_PT = 10;
const BAR_SIDE_PADDING = 12;
const BAR_ITEM_GAP = 6;
const KEYBOARD_BAR_ITEM_GAP = 5;
const TOGGLE_WIDTH = 44;
const ARROWS_COLLAPSED_WIDTH = 64;
const ARROWS_KEYBOARD_WIDTH = 50;
// A phone on its side: the hint line only shows while it says something new.
const COMPACT_HEIGHT = 480;
const ARROWS_TIP_MS = 2500;
const ARROW_ACTIONS: SwipeDirection[] = ['up', 'down', 'left', 'right'];

type SlotKind = 'toggle' | 'flex' | 'arrows';

function slotKind(key: RowKey): SlotKind {
  if (key === 'toggleKeyboard') return 'toggle';
  if (key === 'arrows') return 'arrows';
  return 'flex';
}

function modifierState(state: TermMods, key: 'ctrl' | 'alt'): ModState {
  return state[key];
}

export function TerminalPane({
  sshId,
  fontSize,
  theme,
  accessibilityLabel,
  active = true,
  accessory,
  onCopied,
  ref,
}: TerminalPaneProps): React.JSX.Element {
  const insets = useSafeAreaInsets();
  const window = useWindowDimensions();
  const terminalRef = useRef<TerminalViewHandle>(null);
  const barRef = useRef<ViewInstance>(null);
  const [state, setState] = useState<TermMods>(NO_MODS);
  const [keyboardRequested, setKeyboardActive] = useState(false);
  const keyboardActive = active && keyboardRequested;
  const activeRef = useRef(active);
  activeRef.current = active;
  const [keyboardFrame, setKeyboardFrame] = useState<KeyboardFrame>({
    height: 0,
    visible: false,
    hardware: false,
  });
  const [barBottom, setBarBottom] = useState<number | null>(null);
  const arrowSwipe = useRef<SwipeArrows>(new SwipeArrows());
  const lastArrowPoint = useRef<{ x: number; y: number } | null>(null);
  const arrowGestureActive = useRef(false);
  const pendingRowDx = useRef(0);
  const arrowMoved = useRef(false);
  const [arrowsTip, setArrowsTip] = useState(false);
  const arrowsTipTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const compact = window.height < COMPACT_HEIGHT;
  const sessionReady = active && sshId !== null;
  const sessionReadyRef = useRef(sessionReady);
  sessionReadyRef.current = sessionReady;

  useImperativeHandle(
    ref,
    () => ({
      sendSequence(text) {
        terminalRef.current?.sendSequence(text);
      },
      copySelection() {
        terminalRef.current?.copySelection();
      },
      paste() {
        terminalRef.current?.paste();
      },
    }),
    [],
  );

  const sendKey = useCallback((name: string) => {
    if (sessionReadyRef.current) terminalRef.current?.sendKey(name);
  }, []);

  useEffect(() => {
    if (!active) return;
    const visible = keyboardFrame.visible && !keyboardFrame.hardware;
    setTerminalKeyboard(visible, visible ? keyboardFrame.height : 0);
  }, [active, keyboardFrame]);

  useEffect(() => {
    if (!active) setKeyboardActive(false);
  }, [active]);

  useEffect(
    () => () => {
      if (activeRef.current) resetTerminalKeyboard();
      if (arrowsTipTimer.current !== null) clearTimeout(arrowsTipTimer.current);
    },
    [],
  );

  // A tap on the arrow pad sends nothing; say how it works instead.
  const showArrowsTip = useCallback(() => {
    if (arrowsTipTimer.current !== null) clearTimeout(arrowsTipTimer.current);
    setArrowsTip(true);
    arrowsTipTimer.current = setTimeout(() => {
      arrowsTipTimer.current = null;
      setArrowsTip(false);
    }, ARROWS_TIP_MS);
  }, []);

  const measureBar = useCallback(() => {
    barRef.current?.measureInWindow((_x, y, _width, height) => {
      setBarBottom(y + height);
    });
  }, []);

  useEffect(() => {
    if (keyboardFrame.visible && !keyboardFrame.hardware) {
      measureBar();
    }
  }, [keyboardFrame, measureBar]);

  const keyboardOverlap =
    keyboardFrame.visible && !keyboardFrame.hardware
      ? Math.max(0, keyboardFrame.height)
      : 0;
  const windowHeight = window.height;
  const barDistanceToBottom =
    barBottom === null
      ? Math.max(insets.bottom, 0)
      : Math.max(0, windowHeight - barBottom);
  const keyboardPadding = Math.max(0, keyboardOverlap - barDistanceToBottom);
  const barPaddingBottom =
    keyboardOverlap > 0 ? keyboardPadding : Math.max(insets.bottom, 0);

  const onKeyboardFrame = useCallback((frame: KeyboardFrame) => {
    setKeyboardFrame(frame);
  }, []);

  const onActiveChange = useCallback((focused: boolean) => {
    setKeyboardActive(activeRef.current && focused);
  }, []);

  const pressModifier = (key: 'ctrl' | 'alt') => {
    setState(current => ({ ...current, [key]: cycle(current[key]) }));
  };

  const sendArrows = useCallback(
    (arrows: SwipeDirection[]) => {
      for (const arrow of arrows) {
        arrowMoved.current = true;
        sendKey(arrow);
      }
    },
    [sendKey],
  );

  const arrowResponder = useRef<PanResponderInstance | null>(null);
  if (arrowResponder.current === null) {
    arrowResponder.current = PanResponder.create({
      onStartShouldSetPanResponder: () => sessionReadyRef.current,
      onPanResponderGrant: (_event, gesture) => {
        arrowGestureActive.current = true;
        arrowMoved.current = false;
        arrowSwipe.current.start();
        lastArrowPoint.current = { x: gesture.x0, y: gesture.y0 };
      },
      onPanResponderMove: (_event, gesture) => {
        const last = lastArrowPoint.current ?? {
          x: gesture.moveX,
          y: gesture.moveY,
        };
        const dx = gesture.moveX - last.x;
        const dy = gesture.moveY - last.y;
        lastArrowPoint.current = { x: gesture.moveX, y: gesture.moveY };
        sendArrows(arrowSwipe.current.move(dx, dy, 'any'));
      },
      onPanResponderRelease: () => {
        arrowGestureActive.current = false;
        arrowSwipe.current.start();
        lastArrowPoint.current = null;
        if (!arrowMoved.current) showArrowsTip();
      },
      onPanResponderTerminate: () => {
        arrowGestureActive.current = false;
        arrowSwipe.current.start();
        lastArrowPoint.current = null;
      },
    });
  }

  const rowResponder = useRef<PanResponderInstance | null>(null);
  if (rowResponder.current === null) {
    rowResponder.current = PanResponder.create({
      onStartShouldSetPanResponder: () => false,
      onMoveShouldSetPanResponderCapture: (_event, gesture) => {
        const takesOver =
          sessionReadyRef.current &&
          !arrowGestureActive.current &&
          Math.abs(gesture.dx) > SWIPE_TAKEOVER_PT &&
          Math.abs(gesture.dx) > Math.abs(gesture.dy);
        if (takesOver) pendingRowDx.current = gesture.dx;
        return takesOver;
      },
      onMoveShouldSetPanResponder: (_event, gesture) => {
        const takesOver =
          sessionReadyRef.current &&
          !arrowGestureActive.current &&
          Math.abs(gesture.dx) > SWIPE_TAKEOVER_PT &&
          Math.abs(gesture.dx) > Math.abs(gesture.dy);
        if (takesOver) pendingRowDx.current = gesture.dx;
        return takesOver;
      },
      onPanResponderGrant: (_event, gesture) => {
        arrowSwipe.current.start();
        lastArrowPoint.current = { x: gesture.moveX, y: gesture.moveY };
        const pending = pendingRowDx.current;
        pendingRowDx.current = 0;
        if (pending !== 0) {
          sendArrows(arrowSwipe.current.move(pending, 0, 'horizontal'));
        }
      },
      onPanResponderMove: (_event, gesture) => {
        const last = lastArrowPoint.current ?? {
          x: gesture.moveX,
          y: gesture.moveY,
        };
        const dx = gesture.moveX - last.x;
        lastArrowPoint.current = { x: gesture.moveX, y: gesture.moveY };
        sendArrows(arrowSwipe.current.move(dx, 0, 'horizontal'));
      },
      onPanResponderTerminate: () => {
        arrowSwipe.current.start();
        lastArrowPoint.current = null;
      },
      onPanResponderRelease: () => {
        arrowSwipe.current.start();
        lastArrowPoint.current = null;
      },
    });
  }

  const renderKey = (key: RowKey) => {
    const disabled = !sessionReady;
    switch (key) {
      case 'toggleKeyboard':
        return (
          <TermKeyButton
            accessibilityLabel={
              keyboardActive
                ? terminalBarStrings.hideKeyboard
                : terminalBarStrings.showKeyboard
            }
            disabled={disabled}
            key={key}
            label={
              <Icon
                color={keyboardActive ? 'text' : 'accentText'}
                name={keyboardActive ? 'keyboardHide' : 'keyboard'}
                size={20}
              />
            }
            onPress={() => {
              setKeyboardActive(requested => !requested);
            }}
            testID="terminal-toggle-keyboard"
            variant={keyboardActive ? 'toggleOpen' : 'toggle'}
            style={styles.fillKey}
          />
        );
      case 'esc':
        return (
          <TermKeyButton
            accessibilityLabel={terminalBarStrings.esc}
            disabled={disabled}
            key={key}
            label={terminalBarStrings.esc}
            onPress={() => sendKey('esc')}
            style={styles.fillKey}
            testID="terminal-key-esc"
          />
        );
      case 'ctrl':
      case 'alt':
        return (
          <TermKeyButton
            accessibilityLabel={modA11y(key, modifierState(state, key))}
            disabled={disabled}
            key={key}
            label={terminalBarStrings[key]}
            modifierState={modifierState(state, key)}
            onPress={() => pressModifier(key)}
            style={styles.fillKey}
            testID={`terminal-key-${key}`}
          />
        );
      case 'tab':
        return (
          <TermKeyButton
            accessibilityLabel={terminalBarStrings.tab}
            disabled={disabled}
            key={key}
            label={terminalBarStrings.tab}
            onPress={() => sendKey('tab')}
            style={styles.fillKey}
            testID="terminal-key-tab"
          />
        );
      case 'pipe':
        return (
          <TermKeyButton
            accessibilityLabel={terminalBarStrings.pipeA11y}
            disabled={disabled}
            key={key}
            label={terminalBarStrings.pipe}
            onPress={() => sendKey('|')}
            style={styles.fillKey}
            testID="terminal-key-pipe"
            glyphSize={15}
          />
        );
      case 'tilde':
        return (
          <TermKeyButton
            accessibilityLabel={terminalBarStrings.tildeA11y}
            disabled={disabled}
            key={key}
            label={terminalBarStrings.tilde}
            onPress={() => sendKey('~')}
            style={styles.fillKey}
            testID="terminal-key-tilde"
          />
        );
      case 'arrows':
        return (
          <View
            {...arrowResponder.current?.panHandlers}
            accessible
            accessibilityActions={ARROW_ACTIONS.map(name => ({
              name,
              label: terminalBarStrings.arrowActions[name],
            }))}
            accessibilityLabel={
              keyboardActive
                ? terminalBarStrings.arrowsKeyboardA11y
                : terminalBarStrings.arrowsCollapsedA11y
            }
            accessibilityRole="button"
            accessibilityState={{ disabled }}
            key={key}
            onAccessibilityAction={event => {
              const name = event.nativeEvent.actionName as SwipeDirection;
              if (ARROW_ACTIONS.includes(name)) sendKey(name);
            }}
            style={[
              styles.arrowKey,
              { height: hit.min },
              keyboardActive ? styles.arrowKeyOpen : styles.arrowKeyClosed,
              disabled ? styles.keyDisabled : null,
            ]}
            testID="terminal-key-arrows"
          >
            <Icon color="text" name="dpad" size={26} />
          </View>
        );
    }
  };

  const noMods = state.ctrl === 'none' && state.alt === 'none';
  const hint = arrowsTip
    ? terminalBarStrings.arrowsTip
    : keyboardActive
    ? `${terminalBarStrings.hintKeyboardSwipePrefix} ${modHint(state)}`
    : modHint(state);
  const showHint = !compact || !noMods || arrowsTip;

  return (
    <View style={styles.root}>
      <TerminalView
        accessibilityLabel={accessibilityLabel}
        fontSize={fontSize}
        keyboardActive={keyboardActive}
        modifiers={termMods(state)}
        onActiveChange={onActiveChange}
        onCopied={onCopied}
        onKeyboardFrame={onKeyboardFrame}
        onModifiersConsumed={() => {
          setState(consumeOnce);
        }}
        ref={terminalRef}
        sshId={sshId}
        theme={theme}
      />
      <View
        accessibilityLabel={
          keyboardActive ? terminalBarStrings.keyboardBarA11y : undefined
        }
        accessible={keyboardActive}
        onLayout={measureBar}
        pointerEvents={active ? 'auto' : 'none'}
        ref={barRef}
        style={[
          styles.bar,
          keyboardActive ? styles.keyboardBar : null,
          {
            paddingBottom: barPaddingBottom,
            paddingHorizontal: BAR_SIDE_PADDING,
          },
        ]}
        testID="terminal-key-bar"
      >
        {accessory}
        <View
          {...rowResponder.current.panHandlers}
          style={[
            styles.row,
            { gap: keyboardActive ? KEYBOARD_BAR_ITEM_GAP : BAR_ITEM_GAP },
          ]}
          testID="terminal-key-row"
        >
          {rowKeys(keyboardActive).map(key => (
            <View
              key={key}
              style={[
                { height: hit.min },
                slotKind(key) === 'toggle' ? styles.toggleSlot : null,
                slotKind(key) === 'flex' ? styles.flexSlot : null,
                slotKind(key) === 'arrows'
                  ? keyboardActive
                    ? styles.arrowSlotOpen
                    : styles.arrowSlotClosed
                  : null,
              ]}
            >
              {renderKey(key)}
            </View>
          ))}
        </View>
        {showHint ? (
          <AppText
            color={noMods ? 'text3' : 'accent'}
            mono
            numberOfLines={1}
            style={styles.hint}
            testID="terminal-key-hint"
            weight={noMods ? 400 : 600}
          >
            {hint}
          </AppText>
        ) : (
          <View style={styles.compactGap} />
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: tokens.thumbBg },
  bar: {
    paddingTop: 8,
    backgroundColor: tokens.bg,
  },
  keyboardBar: {
    paddingTop: 8,
    gap: 6,
    borderBottomWidth: 1,
    borderBottomColor: '#1E262C',
  },
  row: { flexDirection: 'row', alignItems: 'center' },
  toggleSlot: { width: TOGGLE_WIDTH },
  flexSlot: { flex: 1, minWidth: 0 },
  arrowSlotClosed: { width: ARROWS_COLLAPSED_WIDTH },
  arrowSlotOpen: { width: ARROWS_KEYBOARD_WIDTH },
  fillKey: { width: '100%' },
  arrowKey: {
    borderRadius: 12,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  arrowKeyClosed: { width: ARROWS_COLLAPSED_WIDTH },
  arrowKeyOpen: { width: ARROWS_KEYBOARD_WIDTH },
  keyDisabled: { opacity: 0.4 },
  hint: { paddingVertical: 4, fontSize: 13, lineHeight: 18 },
  compactGap: { height: 6 },
});
