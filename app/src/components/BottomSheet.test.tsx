import { act, fireEvent, render, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import { useEffect } from 'react';
import {
  Animated,
  Keyboard,
  LayoutAnimation,
  Platform,
  StyleSheet,
  Text,
  TextInput,
} from 'react-native';
import type {
  AndroidKeyboardEvent,
  IOSKeyboardEvent,
  KeyboardEvent,
} from 'react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import { mockWindowDimensions } from '../test/dimensions';
import { BottomSheet } from './BottomSheet';

const baseProps = {
  onClose: jest.fn(),
  title: 'Studio Mac mini',
  subtitle: '100.64.0.8',
  children: <Text>弹层内容</Text>,
};

function flatStyle(testID: string) {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
}

describe('BottomSheet', () => {
  beforeEach(() => {
    baseProps.onClose.mockClear();
  });

  it('renders the title, a body-font subtitle and children', async () => {
    await render(<BottomSheet {...baseProps} />);

    expect(screen.getByText('Studio Mac mini')).toBeOnTheScreen();
    expect(screen.getByText('100.64.0.8')).toBeOnTheScreen();
    expect(screen.getByText('弹层内容')).toBeOnTheScreen();
    // Chinese explanations read poorly in the small mono face.
    expect(
      StyleSheet.flatten(screen.getByText('100.64.0.8').props.style),
    ).toMatchObject({
      color: '#86929A',
      fontSize: 13,
      lineHeight: 19,
      fontFamily: 'Geist-Regular',
    });
    expect(screen.getByRole('header')).toHaveTextContent('Studio Mac mini');
  });

  it('closes once from the backdrop and once from the close button', async () => {
    await render(<BottomSheet {...baseProps} />);

    await fireEvent.press(screen.getAllByLabelText('关闭')[0]);
    expect(baseProps.onClose).toHaveBeenCalledTimes(1);

    await fireEvent.press(screen.getAllByLabelText('关闭')[1]);
    expect(baseProps.onClose).toHaveBeenCalledTimes(2);
  });

  it('uses an interactive, labelled backdrop with a half-black fill', async () => {
    await render(<BottomSheet {...baseProps} />);

    const backdrop = screen.getByTestId('sheet-backdrop');
    expect(backdrop).toHaveProp('accessibilityLabel', '关闭');
    expect(backdrop).toHaveProp('accessibilityRole', 'button');
    expect(flatStyle('sheet-backdrop').backgroundColor).toBe(
      'rgba(0, 0, 0, 0.5)',
    );
    expect(screen.getByTestId('bottom-sheet-surface')).toBeOnTheScreen();
  });

  it('draws the 28 px top corners, token card fill, handle and close circle', async () => {
    await render(
      <BottomSheet
        {...baseProps}
        leading={<Text>图标</Text>}
        testID="device-sheet"
      />,
    );

    expect(flatStyle('device-sheet-surface')).toMatchObject({
      paddingTop: 8,
      paddingHorizontal: 20,
      paddingBottom: 34,
      borderTopLeftRadius: 28,
      borderTopRightRadius: 28,
      backgroundColor: '#151A1E',
    });
    const handle = screen.getByTestId('device-sheet-handle', {
      includeHiddenElements: true,
    });
    expect(handle).toHaveProp(
      'importantForAccessibility',
      'no-hide-descendants',
    );
    expect(StyleSheet.flatten(handle.props.style)).toMatchObject({
      width: 36,
      height: 5,
      borderRadius: 3,
      backgroundColor: '#3A444B',
      alignSelf: 'center',
    });

    expect(flatStyle('device-sheet-close')).toMatchObject({
      width: 44,
      height: 44,
    });
    expect(flatStyle('device-sheet-close-circle')).toMatchObject({
      width: 30,
      height: 30,
      borderRadius: 15,
      backgroundColor: '#252D33',
    });
  });

  it('puts scrollable content in a ScrollView and keeps the footer outside it', async () => {
    await render(
      <BottomSheet
        {...baseProps}
        footer={<Text>固定底栏</Text>}
        scroll
        testID="scrolling-sheet"
      />,
    );

    expect(screen.getByTestId('scrolling-sheet-scroll')).toHaveProp(
      'showsVerticalScrollIndicator',
      false,
    );
    expect(screen.getByText('固定底栏')).toBeOnTheScreen();
    expect(screen.getByTestId('scrolling-sheet-scroll')).not.toContainElement(
      screen.getByText('固定底栏'),
    );
    expect(flatStyle('scrolling-sheet-surface').maxHeight).toBeGreaterThan(0);
  });

  it('hides the handle from screen readers and marks the sheet modal', async () => {
    await render(<BottomSheet {...baseProps} testID="a11y-sheet" />);

    expect(screen.getByTestId('a11y-sheet')).toHaveProp(
      'accessibilityViewIsModal',
      true,
    );
    expect(
      screen.getByTestId('a11y-sheet-handle', {
        includeHiddenElements: true,
      }),
    ).toHaveProp('importantForAccessibility', 'no-hide-descendants');
  });
});

type KeyboardListener = (event: KeyboardEvent) => void;

function captureKeyboard() {
  const listeners = new Map<string, KeyboardListener[]>();
  const removed: string[] = [];
  jest.spyOn(Keyboard, 'addListener').mockImplementation(((
    name: string,
    listener: KeyboardListener,
  ) => {
    listeners.set(name, [...(listeners.get(name) ?? []), listener]);
    return {
      remove: () => {
        removed.push(name);
        listeners.set(
          name,
          (listeners.get(name) ?? []).filter(other => other !== listener),
        );
      },
    };
  }) as unknown as typeof Keyboard.addListener);
  return {
    removed,
    events: () =>
      [...listeners.entries()]
        .filter(([, list]) => list.length > 0)
        .map(([name]) => name)
        .sort(),
    emit: (name: string, event: KeyboardEvent) =>
      act(async () => {
        (listeners.get(name) ?? []).forEach(listener => listener(event));
      }),
  };
}

function iosKeyboard(screenY: number, height: number): IOSKeyboardEvent {
  return {
    duration: 250,
    easing: 'keyboard',
    endCoordinates: { screenX: 0, screenY, width: 390, height },
    startCoordinates: { screenX: 0, screenY: 844, width: 390, height },
    isEventFromThisApp: true,
  };
}

// RN reports Android's height above the navigation bar; screenY is the IME top.
function androidKeyboard(
  screenY: number,
  height: number,
): AndroidKeyboardEvent {
  return {
    duration: 0,
    easing: 'keyboard',
    endCoordinates: { screenX: 0, screenY, width: 390, height },
  };
}

const PORTRAIT_INSETS = { top: 47, bottom: 34, left: 0, right: 0 };

async function renderWithInsets(ui: ReactElement, insets = PORTRAIT_INSETS) {
  await render(
    <SafeAreaProvider
      initialMetrics={{
        frame: { x: 0, y: 0, width: 390, height: 844 },
        insets,
      }}
    >
      {ui}
    </SafeAreaProvider>,
  );
}

async function layout(testID: string, height: number) {
  await fireEvent(screen.getByTestId(testID), 'layout', {
    nativeEvent: { layout: { x: 0, y: 0, width: 350, height } },
  });
}

const fieldMounts = jest.fn();

function Field() {
  useEffect(() => {
    fieldMounts();
  }, []);
  return <TextInput testID="kb-input" />;
}

function formSheet(scroll = false) {
  return (
    <BottomSheet
      footer={<Text>连接</Text>}
      onClose={jest.fn()}
      scroll={scroll}
      testID="kb-sheet"
      title="输入密码"
    >
      <Field />
    </BottomSheet>
  );
}

describe('BottomSheet with the software keyboard', () => {
  let animate: jest.SpyInstance;

  beforeEach(() => {
    fieldMounts.mockClear();
    jest.replaceProperty(Platform, 'OS', 'ios');
    mockWindowDimensions({ width: 390, height: 844 });
    animate = jest
      .spyOn(LayoutAnimation, 'configureNext')
      .mockImplementation(() => undefined);
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('lifts the whole iOS sheet 300 pt above the keyboard and leaves the backdrop alone', async () => {
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet());
    expect(keyboard.events()).toEqual([
      'keyboardWillChangeFrame',
      'keyboardWillHide',
    ]);
    const backdrop = flatStyle('kb-sheet-backdrop');
    expect(backdrop).toMatchObject({ position: 'absolute', top: 0, bottom: 0 });
    expect(flatStyle('kb-sheet').paddingBottom).toBe(0);

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(544, 300));

    expect(flatStyle('kb-sheet')).toMatchObject({
      position: 'absolute',
      bottom: 0,
      justifyContent: 'flex-end',
      paddingBottom: 300,
    });
    expect(flatStyle('kb-sheet-backdrop')).toEqual(backdrop);
    expect(screen.getByTestId('kb-sheet-surface')).toContainElement(
      screen.getByText('连接'),
    );
    // The layout change rides the keyboard's own duration and curve.
    expect(animate).toHaveBeenCalledTimes(1);
    expect(animate).toHaveBeenCalledWith({
      duration: 250,
      update: { duration: 250, type: 'keyboard' },
    });
  });

  it('puts the iOS sheet back when the keyboard hides', async () => {
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet());
    const surface = flatStyle('kb-sheet-surface');
    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(544, 300));

    // iOS moves the frame below the window first, then posts willHide.
    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(844, 300));
    await keyboard.emit('keyboardWillHide', iosKeyboard(844, 300));

    expect(flatStyle('kb-sheet').paddingBottom).toBe(0);
    expect(flatStyle('kb-sheet-surface')).toEqual(surface);
    expect(animate).toHaveBeenCalledTimes(2);
  });

  it('lifts the Android sheet from keyboardDidShow, since edge-to-edge windows do not shrink', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet(), { ...PORTRAIT_INSETS, bottom: 48 });
    expect(keyboard.events()).toEqual(['keyboardDidHide', 'keyboardDidShow']);
    expect(flatStyle('kb-sheet-surface').paddingBottom).toBe(56);

    await keyboard.emit('keyboardDidShow', androidKeyboard(544, 300 - 48));
    expect(flatStyle('kb-sheet').paddingBottom).toBe(300);
    expect(flatStyle('kb-sheet-surface').paddingBottom).toBe(16);

    await keyboard.emit('keyboardDidHide', androidKeyboard(844, 0));
    expect(flatStyle('kb-sheet').paddingBottom).toBe(0);
    expect(flatStyle('kb-sheet-surface').paddingBottom).toBe(56);
    // Android reports no duration, so there is nothing to follow.
    expect(animate).not.toHaveBeenCalled();
  });

  it('drops the home indicator from the bottom padding while the keyboard is up', async () => {
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet());
    expect(flatStyle('kb-sheet-surface').paddingBottom).toBe(34 + 8);

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(544, 300));

    expect(flatStyle('kb-sheet-surface').paddingBottom).toBe(16);
  });

  it.each([
    ['a hardware keyboard parked below the window', iosKeyboard(844, 300)],
    ['a floating iPad keyboard', iosKeyboard(400, 260)],
  ])('does not move for %s', async (_name, event) => {
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet());

    await keyboard.emit('keyboardWillChangeFrame', event);

    expect(flatStyle('kb-sheet').paddingBottom).toBe(0);
    expect(flatStyle('kb-sheet-surface').paddingBottom).toBe(34 + 8);
    expect(animate).not.toHaveBeenCalled();
  });

  it('caps a landscape sheet above a 200 pt keyboard and scrolls it as a whole', async () => {
    mockWindowDimensions({ width: 852, height: 393 });
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet(), {
      top: 0,
      bottom: 21,
      left: 59,
      right: 59,
    });
    await layout('kb-sheet-header', 44);
    await layout('kb-sheet-footer', 108);

    // Without the keyboard the header and footer stay pinned.
    const frame = () => screen.getByTestId('kb-sheet-frame');
    expect(frame()).toHaveProp('scrollEnabled', false);
    expect(
      StyleSheet.flatten(frame().props.contentContainerStyle).maxHeight,
    ).toBe(393 - 12 - 8 - 34);

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(193, 200));

    expect(flatStyle('kb-sheet').paddingBottom).toBe(200);
    const maxHeight = flatStyle('kb-sheet-surface').maxHeight;
    expect(maxHeight).toBeLessThanOrEqual(393 - 200);
    expect(maxHeight).toBe(393 - 200 - 12);
    // 157 pt cannot hold the handle, header, footer and a field: the title,
    // the field and the primary button all scroll together instead.
    expect(frame()).toHaveProp('scrollEnabled', true);
    expect(
      StyleSheet.flatten(frame().props.contentContainerStyle).maxHeight,
    ).toBeUndefined();
    expect(frame()).toContainElement(screen.getByRole('header'));
    expect(frame()).toContainElement(screen.getByText('连接'));
    expect(screen.getByTestId('kb-sheet-scroll')).toContainElement(
      screen.getByTestId('kb-input'),
    );
    expect(frame()).toHaveProp('keyboardShouldPersistTaps', 'handled');
    expect(screen.getByTestId('kb-sheet-scroll')).toHaveProp(
      'keyboardShouldPersistTaps',
      'handled',
    );
    // Same element tree in both modes: the focused field is not remounted.
    expect(fieldMounts).toHaveBeenCalledTimes(1);
  });

  it('keeps the header and footer pinned above a portrait keyboard and scrolls the content', async () => {
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet(true));
    await layout('kb-sheet-header', 44);
    await layout('kb-sheet-footer', 108);

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(508, 336));

    const available = 844 - 336 - 47 - 12;
    expect(flatStyle('kb-sheet-surface').maxHeight).toBe(available);
    const frame = screen.getByTestId('kb-sheet-frame');
    expect(frame).toHaveProp('scrollEnabled', false);
    expect(
      StyleSheet.flatten(frame.props.contentContainerStyle).maxHeight,
    ).toBe(available - 8 - 16);
    expect(screen.getByTestId('kb-sheet-scroll')).toContainElement(
      screen.getByTestId('kb-input'),
    );
    expect(screen.getByTestId('kb-sheet-scroll')).not.toContainElement(
      screen.getByText('连接'),
    );
  });

  it('starts lifted when the keyboard is already up', async () => {
    jest.spyOn(Keyboard, 'isVisible').mockReturnValue(true);
    jest
      .spyOn(Keyboard, 'metrics')
      .mockReturnValue({ screenX: 0, screenY: 544, width: 390, height: 300 });
    captureKeyboard();
    await renderWithInsets(formSheet());

    expect(flatStyle('kb-sheet').paddingBottom).toBe(300);
    expect(animate).not.toHaveBeenCalled();
  });

  it('stops listening on unmount', async () => {
    const keyboard = captureKeyboard();
    await renderWithInsets(formSheet());
    await screen.unmount();

    expect(keyboard.removed.sort()).toEqual([
      'keyboardWillChangeFrame',
      'keyboardWillHide',
    ]);
    expect(keyboard.events()).toEqual([]);
  });
});

type Point = { y: number; t: number; x?: number };

// A single-finger responder event in the shape PanResponder reads.
function touch(current: Point, previous: Point) {
  return {
    nativeEvent: {
      pageX: current.x ?? 0,
      pageY: current.y,
      timestamp: current.t,
      touches: [],
      changedTouches: [],
    },
    touchHistory: {
      numberActiveTouches: 1,
      indexOfSingleActiveTouch: 0,
      mostRecentTimeStamp: current.t,
      touchBank: [
        {
          touchActive: true,
          startPageX: 0,
          startPageY: 0,
          startTimeStamp: 0,
          currentPageX: current.x ?? 0,
          currentPageY: current.y,
          currentTimeStamp: current.t,
          previousPageX: previous.x ?? 0,
          previousPageY: previous.y,
          previousTimeStamp: previous.t,
        },
      ],
    },
  };
}

// The responder system asks the capture phase (where PanResponder tracks the
// move) before the bubble phase (where the sheet decides to take it).
function negotiate(
  grab: ReturnType<typeof screen.getByTestId>,
  event: ReturnType<typeof touch>,
): boolean {
  expect(grab.props.onMoveShouldSetResponderCapture(event)).toBe(false);
  return grab.props.onMoveShouldSetResponder(event);
}

// Claims the gesture after a 10 pt move, then pulls a further `dy` over `ms`.
async function pull(testID: string, dy: number, ms: number) {
  const grab = screen.getByTestId(testID);
  const start = { y: 0, t: 0 };
  const claim = { y: 10, t: 16 };
  const end = { y: 10 + dy, t: 16 + ms };
  let claimed = false;
  await act(async () => {
    claimed = negotiate(grab, touch(claim, start));
    if (claimed) {
      grab.props.onResponderGrant(touch(claim, start));
      grab.props.onResponderMove(touch(end, claim));
      grab.props.onResponderRelease(touch(end, claim));
    }
  });
  return claimed;
}

describe('BottomSheet motion and dismissal', () => {
  let restoreDimensions: () => void;

  beforeEach(() => {
    jest.useFakeTimers();
    restoreDimensions = mockWindowDimensions({ width: 390, height: 844 });
  });

  afterEach(() => {
    restoreDimensions();
    jest.restoreAllMocks();
    jest.useRealTimers();
  });

  it('fades the backdrop in and slides the panel up from below the window', async () => {
    const timing = jest.spyOn(Animated, 'timing');
    await render(<BottomSheet {...baseProps} testID="enter" />);

    expect(timing).toHaveBeenCalledTimes(1);
    expect(timing).toHaveBeenCalledWith(
      expect.any(Animated.Value),
      expect.objectContaining({
        toValue: 1,
        duration: 250,
        useNativeDriver: true,
      }),
    );
    // First frame: the panel is a full window below its place and the
    // backdrop is clear; the native driver runs both to rest.
    expect(flatStyle('enter-panel').transform).toEqual([{ translateY: 844 }]);
    const scrim = screen.getByTestId('enter-backdrop').parent;
    expect(StyleSheet.flatten(scrim?.props.style).opacity).toBe(0);
    expect(scrim).toHaveProp('pointerEvents', 'box-none');
  });

  it('closes once after a pull past 96 pt on the handle and header', async () => {
    const onClose = jest.fn();
    await render(
      <BottomSheet {...baseProps} onClose={onClose} testID="pull" />,
    );
    const grab = screen.getByTestId('pull-grab');
    expect(grab).toContainElement(
      screen.getByTestId('pull-handle', { includeHiddenElements: true }),
    );
    expect(grab).toContainElement(screen.getByTestId('pull-header'));
    expect(grab).not.toContainElement(screen.getByText('弹层内容'));

    const timing = jest.spyOn(Animated, 'timing');
    expect(await pull('pull-grab', 120, 400)).toBe(true);
    expect(timing).toHaveBeenLastCalledWith(
      expect.any(Animated.Value),
      expect.objectContaining({ toValue: 844, useNativeDriver: true }),
    );
    expect(onClose).not.toHaveBeenCalled();

    await act(() => jest.advanceTimersByTimeAsync(250));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes on a quick downward flick', async () => {
    const onClose = jest.fn();
    await render(
      <BottomSheet {...baseProps} onClose={onClose} testID="flick" />,
    );

    expect(await pull('flick-grab', 30, 20)).toBe(true);
    await act(() => jest.advanceTimersByTimeAsync(250));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('springs back from a short, slow pull', async () => {
    const onClose = jest.fn();
    await render(
      <BottomSheet {...baseProps} onClose={onClose} testID="short" />,
    );
    const spring = jest.spyOn(Animated, 'spring');

    expect(await pull('short-grab', 40, 400)).toBe(true);
    expect(spring).toHaveBeenCalledWith(
      expect.any(Animated.Value),
      expect.objectContaining({ toValue: 0, useNativeDriver: true }),
    );
    await act(() => jest.advanceTimersByTimeAsync(1000));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('leaves sideways and upward moves to the content', async () => {
    await render(<BottomSheet {...baseProps} testID="sideways" />);
    const grab = screen.getByTestId('sideways-grab');
    const start = { y: 0, t: 0 };

    expect(negotiate(grab, touch({ x: 40, y: 10, t: 16 }, start))).toBe(false);
    expect(negotiate(grab, touch({ y: -20, t: 32 }, start))).toBe(false);
  });

  it('does not follow a pull when it is not dismissible', async () => {
    const onClose = jest.fn();
    await render(
      <BottomSheet
        {...baseProps}
        dismissible={false}
        onClose={onClose}
        testID="locked"
      />,
    );

    expect(await pull('locked-grab', 200, 100)).toBe(false);
    await act(() => jest.advanceTimersByTimeAsync(250));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('keeps the sheet to a centred 540 pt column on wide windows', async () => {
    restoreDimensions();
    restoreDimensions = mockWindowDimensions({ width: 1024, height: 768 });
    await render(<BottomSheet {...baseProps} testID="wide" />);

    expect(flatStyle('wide-panel')).toMatchObject({
      width: '100%',
      alignItems: 'center',
    });
    expect(flatStyle('wide-surface')).toMatchObject({
      width: '100%',
      maxWidth: 540,
    });
  });
});
