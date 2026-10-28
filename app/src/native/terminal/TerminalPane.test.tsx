import { createRef } from 'react';
import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
} from '@testing-library/react-native';
import { Platform, StyleSheet, View as AccessoryView } from 'react-native';
import { mockWindowDimensions } from '../../test/dimensions';
import { Commands } from '../specs/RsTerminalViewNativeComponent';
import type { NativeProps } from '../specs/RsTerminalViewNativeComponent';
import { TerminalPane } from './TerminalPane';
import type { TerminalHandle } from './TerminalView';
import { resetTerminalKeyboard, useTerminalKeyboard } from './terminalKeyboard';
import * as terminalKeyboard from './terminalKeyboard';

let mockLastProps: NativeProps;
const mockNativeHandle = {};

jest.mock('../specs/RsTerminalViewNativeComponent', () => {
  const React = require('react');
  const { View } = require('react-native');
  return {
    __esModule: true,
    default: function MockNativeTerminal(
      p: NativeProps & { ref?: React.Ref<unknown> },
    ) {
      mockLastProps = p;
      React.useImperativeHandle(p.ref, () => mockNativeHandle);
      return <View testID="native-terminal" />;
    },
    Commands: {
      sendSequence: jest.fn(),
      sendKey: jest.fn(),
      copySelection: jest.fn(),
      paste: jest.fn(),
    },
  };
});

const props = {
  sshId: 'ssh-1',
  fontSize: 13,
  theme: 'night' as const,
  accessibilityLabel: '终端输出',
};

beforeEach(() => {
  jest.clearAllMocks();
  resetTerminalKeyboard();
});

function barStyle() {
  return StyleSheet.flatten(screen.getByTestId('terminal-key-bar').props.style);
}

function touchEvent(x: number, y: number, timestamp: number) {
  return {
    nativeEvent: { pageX: x, pageY: y, touches: [{ pageX: x, pageY: y }] },
    touchHistory: {
      numberActiveTouches: 1,
      indexOfSingleActiveTouch: 0,
      mostRecentTimeStamp: timestamp,
      touchBank: [
        {
          touchActive: true,
          startPageX: 0,
          startPageY: 0,
          startTimeStamp: 0,
          currentPageX: x,
          currentPageY: y,
          currentTimeStamp: timestamp,
          previousPageX: 0,
          previousPageY: 0,
          previousTimeStamp: 0,
        },
      ],
    },
  };
}

it('shows the collapsed bar without pipe and tilde', async () => {
  await render(<TerminalPane {...props} />);
  expect(screen.getByLabelText('显示键盘')).toBeOnTheScreen();
  expect(screen.getByLabelText('esc')).toBeOnTheScreen();
  expect(screen.getByLabelText('方向键，按住滑动')).toBeOnTheScreen();
  expect(screen.queryByLabelText('竖线')).toBeNull();
  expect(screen.queryByLabelText('波浪号')).toBeNull();
  expect(screen.getByText('修饰键点两下锁定')).toBeOnTheScreen();
});

it('opens the keyboard row and updates the native keyboard flag', async () => {
  await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('显示键盘'));

  expect(mockLastProps.keyboardActive).toBe(true);
  expect(screen.getByLabelText('收起键盘')).toBeOnTheScreen();
  expect(screen.getByLabelText('竖线')).toBeOnTheScreen();
  expect(screen.getByLabelText('波浪号')).toBeOnTheScreen();
  expect(screen.getByLabelText('方向键，按住拖动')).toBeOnTheScreen();
  expect(screen.getByTestId('terminal-key-bar').props.accessible).toBe(true);
  expect(
    screen.getByText('在按键行上左右滑动 = 方向键 修饰键点两下锁定'),
  ).toBeOnTheScreen();
});

it('cycles ctrl through once and lock without consuming a lock', async () => {
  await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('ctrl，未按下'));
  expect(mockLastProps.modifiers).toBe(1);
  expect(screen.getByLabelText('ctrl，单次，下一个键后松开')).toBeOnTheScreen();
  expect(screen.getByText('ctrl 单次')).toBeOnTheScreen();

  await act(async () => {
    mockLastProps.onModifiersConsumed?.({
      nativeEvent: { mods: 1 },
    } as never);
  });
  expect(screen.getByLabelText('ctrl，未按下')).toBeOnTheScreen();

  await fireEvent.press(screen.getByLabelText('ctrl，未按下'));
  await fireEvent.press(screen.getByLabelText('ctrl，单次，下一个键后松开'));
  expect(screen.getByLabelText('ctrl，已锁定')).toBeOnTheScreen();
  expect(mockLastProps.modifiers).toBe(1);
  expect(
    screen.getByLabelText('ctrl，已锁定').props.accessibilityState,
  ).toEqual({ disabled: false, selected: true });

  await act(async () => {
    mockLastProps.onModifiersConsumed?.({
      nativeEvent: { mods: 1 },
    } as never);
  });
  expect(screen.getByLabelText('ctrl，已锁定')).toBeOnTheScreen();
});

it('combines ctrl and alt in the hint and native mask', async () => {
  await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('ctrl，未按下'));
  await fireEvent.press(screen.getByLabelText('alt，未按下'));
  await fireEvent.press(screen.getByLabelText('alt，单次，下一个键后松开'));
  expect(mockLastProps.modifiers).toBe(3);
  expect(screen.getByText('ctrl 单次 · alt 已锁定')).toBeOnTheScreen();
});

it('sends esc, tab, pipe and tilde through sendKey', async () => {
  await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('esc'));
  await fireEvent.press(screen.getByLabelText('tab'));
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'esc');
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'tab');

  await fireEvent.press(screen.getByLabelText('显示键盘'));
  await fireEvent.press(screen.getByLabelText('竖线'));
  await fireEvent.press(screen.getByLabelText('波浪号'));
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, '|');
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, '~');
});

it('only sends arrows while dragging the arrow pad', async () => {
  await render(<TerminalPane {...props} />);
  const arrows = screen.getByTestId('terminal-key-arrows');
  await fireEvent(arrows, 'responderGrant', touchEvent(0, 0, 1));
  await fireEvent(arrows, 'responderRelease', touchEvent(0, 0, 2));
  expect(Commands.sendKey).not.toHaveBeenCalled();

  await fireEvent(arrows, 'responderGrant', touchEvent(0, 0, 3));
  await fireEvent(arrows, 'responderMove', touchEvent(50, 0, 4));
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'right');
  expect(Commands.sendKey).toHaveBeenCalledTimes(2);
});

it('does not take over a plain tap on the key row', async () => {
  await render(<TerminalPane {...props} />);
  const row = screen.getByTestId('terminal-key-row');
  const shouldSet = row.props.onMoveShouldSetResponderCapture;
  expect(typeof shouldSet).toBe('function');
  expect(
    shouldSet({
      nativeEvent: { touches: [{ pageX: 0, pageY: 0 }] },
      touchHistory: {
        numberActiveTouches: 1,
        indexOfSingleActiveTouch: 0,
        mostRecentTimeStamp: 2,
        touchBank: [
          {
            touchActive: true,
            startPageX: 0,
            startPageY: 0,
            startTimeStamp: 1,
            currentPageX: 8,
            currentPageY: 0,
            currentTimeStamp: 2,
            previousPageX: 0,
            previousPageY: 0,
            previousTimeStamp: 1,
          },
        ],
      },
    }),
  ).toBe(false);
  expect(row).toBeOnTheScreen();
});

it('sends horizontal arrows after the key row takes over', async () => {
  await render(<TerminalPane {...props} />);
  const row = screen.getByTestId('terminal-key-row');
  const capture = row.props.onMoveShouldSetResponderCapture;
  const shouldTakeOver = capture({
    nativeEvent: { touches: [{ pageX: 0, pageY: 0 }] },
    touchHistory: {
      numberActiveTouches: 1,
      indexOfSingleActiveTouch: 0,
      mostRecentTimeStamp: 2,
      touchBank: [
        {
          touchActive: true,
          startPageX: 0,
          startPageY: 0,
          startTimeStamp: 1,
          currentPageX: 50,
          currentPageY: 0,
          currentTimeStamp: 2,
          previousPageX: 0,
          previousPageY: 0,
          previousTimeStamp: 1,
        },
      ],
    },
  });
  expect(shouldTakeOver).toBe(true);
  expect(Commands.sendKey).not.toHaveBeenCalled();

  await fireEvent(row, 'responderGrant', touchEvent(50, 0, 3));
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'right');
  expect(Commands.sendKey).toHaveBeenCalledTimes(2);
});

it('follows native active changes back to the collapsed bar', async () => {
  await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('显示键盘'));
  expect(mockLastProps.keyboardActive).toBe(true);

  await act(async () => {
    mockLastProps.onActiveChange?.({ nativeEvent: { active: false } } as never);
  });
  expect(mockLastProps.keyboardActive).toBe(false);
  expect(screen.getByLabelText('显示键盘')).toBeOnTheScreen();
});

it('forwards the page handle to native commands', async () => {
  const ref = createRef<TerminalHandle>();
  await render(<TerminalPane {...props} ref={ref} />);
  await act(async () => {
    ref.current?.sendSequence('ls');
    ref.current?.paste();
    ref.current?.copySelection();
  });
  expect(Commands.sendSequence).toHaveBeenCalledWith(mockNativeHandle, 'ls');
  expect(Commands.paste).toHaveBeenCalledWith(mockNativeHandle);
  expect(Commands.copySelection).toHaveBeenCalledWith(mockNativeHandle);
});

it('disables bar buttons without an ssh id', async () => {
  await render(<TerminalPane {...props} sshId={null} />);
  for (const label of [
    '显示键盘',
    'esc',
    'ctrl，未按下',
    'alt，未按下',
    'tab',
  ]) {
    expect(screen.getByLabelText(label)).toBeDisabled();
  }
  expect(screen.getByLabelText('方向键，按住滑动')).toHaveProp(
    'accessibilityState',
    { disabled: true },
  );
});

it('writes keyboard visibility and pads above the system keyboard', async () => {
  const keyboard = await renderHook(() => useTerminalKeyboard());
  await render(<TerminalPane {...props} />);

  await act(async () => {
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 336, visible: true, hardware: false },
    } as never);
  });
  expect(keyboard.result.current.visible).toBe(true);
  expect(barStyle().paddingBottom).toBeGreaterThan(0);
});

it('does not pad for a hardware keyboard', async () => {
  const keyboard = await renderHook(() => useTerminalKeyboard());
  await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('显示键盘'));
  await act(async () => {
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 336, visible: true, hardware: true },
    } as never);
  });
  expect(keyboard.result.current).toEqual({ visible: false, overlap: 0 });
  expect(barStyle().paddingBottom).toBe(0);
});

it('resets the keyboard store when the pane unmounts', async () => {
  const keyboard = await renderHook(() => useTerminalKeyboard());
  const pane = await render(<TerminalPane {...props} />);
  await act(async () => {
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 336, visible: true, hardware: false },
    } as never);
  });
  expect(keyboard.result.current.visible).toBe(true);

  await pane.unmount();
  expect(keyboard.result.current).toEqual({ visible: false, overlap: 0 });
});

it('keeps an inactive pane unfocused and never writes the keyboard store', async () => {
  terminalKeyboard.setTerminalKeyboard(true, 210);
  const keyboard = await renderHook(() => useTerminalKeyboard());
  const setKeyboard = jest.spyOn(terminalKeyboard, 'setTerminalKeyboard');
  const resetKeyboard = jest.spyOn(terminalKeyboard, 'resetTerminalKeyboard');
  const pane = await render(<TerminalPane {...props} active={false} />);

  for (const label of [
    '显示键盘',
    'esc',
    'ctrl，未按下',
    'alt，未按下',
    'tab',
  ]) {
    expect(screen.getByLabelText(label)).toBeDisabled();
    await fireEvent.press(screen.getByLabelText(label));
  }
  expect(screen.getByTestId('terminal-key-bar')).toHaveProp(
    'pointerEvents',
    'none',
  );
  expect(screen.getByTestId('terminal-key-arrows')).toHaveProp(
    'accessibilityState',
    { disabled: true },
  );
  expect(mockLastProps.keyboardActive).toBe(false);
  expect(Commands.sendKey).not.toHaveBeenCalled();

  await act(async () => {
    mockLastProps.onActiveChange?.({ nativeEvent: { active: true } } as never);
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 336, visible: true, hardware: false },
    } as never);
  });
  expect(mockLastProps.keyboardActive).toBe(false);
  await pane.unmount();
  expect(keyboard.result.current).toEqual({ visible: true, overlap: 210 });
  expect(setKeyboard).not.toHaveBeenCalled();
  expect(resetKeyboard).not.toHaveBeenCalled();
  setKeyboard.mockRestore();
  resetKeyboard.mockRestore();
});

it('drops keyboard focus on deactivation without resetting another pane’s store', async () => {
  const keyboard = await renderHook(() => useTerminalKeyboard());
  const pane = await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('显示键盘'));
  expect(mockLastProps.keyboardActive).toBe(true);

  await act(async () => terminalKeyboard.setTerminalKeyboard(true, 210));
  await pane.rerender(<TerminalPane {...props} active={false} />);
  expect(mockLastProps.keyboardActive).toBe(false);
  expect(keyboard.result.current).toEqual({ visible: true, overlap: 210 });
  await act(async () => {
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 0, visible: false, hardware: false },
    } as never);
  });
  expect(keyboard.result.current).toEqual({ visible: true, overlap: 210 });
  await pane.unmount();
  expect(keyboard.result.current).toEqual({ visible: true, overlap: 210 });
});

it('reenables the bar after activation without reopening the keyboard', async () => {
  const pane = await render(<TerminalPane {...props} />);
  await fireEvent.press(screen.getByLabelText('显示键盘'));
  await pane.rerender(<TerminalPane {...props} active={false} />);
  await pane.rerender(<TerminalPane {...props} active />);
  expect(mockLastProps.keyboardActive).toBe(false);
  expect(screen.getByLabelText('显示键盘')).toBeEnabled();
  await fireEvent.press(screen.getByLabelText('显示键盘'));
  expect(mockLastProps.keyboardActive).toBe(true);
  await fireEvent.press(screen.getByLabelText('esc'));
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'esc');
});

it('renders the accessory above the key row inside the keyboard padding', async () => {
  await render(
    <TerminalPane
      {...props}
      accessory={<AccessoryView testID="snippet-accessory" />}
    />,
  );
  const accessory = screen.getByTestId('snippet-accessory');
  const row = screen.getByTestId('terminal-key-row');
  const bar = screen.getByTestId('terminal-key-bar');
  expect(bar.children.indexOf(accessory)).toBeGreaterThanOrEqual(0);
  expect(bar.children.indexOf(accessory)).toBeLessThan(
    bar.children.indexOf(row),
  );

  await fireEvent.press(screen.getByLabelText('显示键盘'));
  await act(async () => {
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 336, visible: true, hardware: false },
    } as never);
  });
  expect(screen.getByTestId('snippet-accessory')).toBe(accessory);
  expect(barStyle().paddingBottom).toBeGreaterThan(0);
  expect(bar.children.indexOf(accessory)).toBeLessThan(
    bar.children.indexOf(row),
  );
});

it('explains a tap on the arrow pad and clears the tip again', async () => {
  jest.useFakeTimers();
  try {
    await render(<TerminalPane {...props} />);
    const arrows = screen.getByTestId('terminal-key-arrows');
    await fireEvent(arrows, 'responderGrant', touchEvent(0, 0, 1));
    await fireEvent(arrows, 'responderRelease', touchEvent(0, 0, 2));
    // T-audit: a tap did nothing and said nothing.
    expect(screen.getByText('方向键要按住往上下左右滑')).toBeOnTheScreen();
    await act(async () => {
      jest.advanceTimersByTime(2500);
    });
    expect(screen.queryByText('方向键要按住往上下左右滑')).toBeNull();
    expect(screen.getByText('修饰键点两下锁定')).toBeOnTheScreen();
  } finally {
    jest.useRealTimers();
  }
});

it('moves with screen-reader actions on the arrow pad', async () => {
  await render(<TerminalPane {...props} />);
  const arrows = screen.getByTestId('terminal-key-arrows');
  expect(arrows.props.accessibilityActions).toEqual([
    { name: 'up', label: '上' },
    { name: 'down', label: '下' },
    { name: 'left', label: '左' },
    { name: 'right', label: '右' },
  ]);
  await fireEvent(arrows, 'accessibilityAction', {
    nativeEvent: { actionName: 'up' },
  });
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'up');
});

it('drops the idle hint row when the phone is on its side', async () => {
  const restore = mockWindowDimensions({ width: 844, height: 390 });
  try {
    await render(<TerminalPane {...props} />);
    // T27: the idle hint costs a line the terminal needs in landscape.
    expect(screen.queryByText('修饰键点两下锁定')).toBeNull();
    await fireEvent.press(screen.getByLabelText('ctrl，未按下'));
    expect(screen.getByText('ctrl 单次')).toBeOnTheScreen();
  } finally {
    restore();
  }
});

it.each([
  ['ios', 44],
  ['android', 48],
] as const)('sizes the bar keys to the %s touch target', async (os, size) => {
  jest.replaceProperty(Platform, 'OS', os);
  await render(<TerminalPane {...props} />);
  // T28
  expect(
    StyleSheet.flatten(screen.getByTestId('terminal-key-esc').props.style)
      .height,
  ).toBe(size);
  expect(
    StyleSheet.flatten(screen.getByTestId('terminal-key-arrows').props.style)
      .height,
  ).toBe(size);
  jest.restoreAllMocks();
});

it('forwards copy feedback from the native view', async () => {
  const onCopied = jest.fn();
  await render(<TerminalPane {...props} onCopied={onCopied} />);
  await act(async () => {
    (
      mockLastProps as unknown as {
        onCopied?: (e: { nativeEvent: { length: number } }) => void;
      }
    ).onCopied?.({ nativeEvent: { length: 5 } });
  });
  // T05
  expect(onCopied).toHaveBeenCalledTimes(1);
});

it('dims a bar key while it is pressed on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  await render(<TerminalPane {...props} />);
  const esc = screen.getByTestId('terminal-key-esc');
  expect(StyleSheet.flatten(esc.props.style).opacity).toBeUndefined();
  // T03: the key answers the touch; Android gets the ripple instead.
  await fireEvent(esc, 'responderGrant', {
    persist: () => {},
    nativeEvent: {},
    currentTarget: 1,
  });
  expect(StyleSheet.flatten(esc.props.style).opacity).toBe(0.6);
  jest.restoreAllMocks();
});
