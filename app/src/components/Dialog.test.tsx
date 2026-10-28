import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { useEffect } from 'react';
import {
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
import { mockWindowDimensions } from '../test/dimensions';
import { Dialog } from './Dialog';

describe('Dialog', () => {
  it('renders nothing while invisible', async () => {
    await render(
      <Dialog
        actions={[{ label: '知道了', onPress: jest.fn() }]}
        title="隐藏标题"
        visible={false}
      />,
    );

    expect(screen.queryByText('隐藏标题')).toBeNull();
    expect(screen.queryByTestId('dialog-card')).toBeNull();
  });

  it('shows title, message, custom content and vertical full-width actions', async () => {
    const first = jest.fn();
    const second = jest.fn();
    await render(
      <Dialog
        actions={[
          { label: '允许', onPress: first },
          { label: '拒绝', onPress: second, variant: 'secondary' },
        ]}
        message="允许这台设备控制？"
        title="控制请求"
        visible
      >
        <Text>请求方：Studio</Text>
      </Dialog>,
    );

    expect(screen.getByRole('header')).toHaveTextContent('控制请求');
    expect(screen.getByText('允许这台设备控制？')).toBeOnTheScreen();
    expect(screen.getByText('请求方：Studio')).toBeOnTheScreen();
    expect(screen.getAllByRole('button')).toHaveLength(2);

    await fireEvent.press(screen.getByText('允许'));
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();

    const cardStyle = StyleSheet.flatten(
      screen.getByTestId('dialog-card').props.style,
    );
    expect(cardStyle).toMatchObject({
      width: 400,
      borderRadius: 22,
      borderWidth: 1,
      borderColor: '#262E34',
      backgroundColor: 'rgba(21, 26, 30, 0.97)',
      padding: 20,
      gap: 12,
    });
  });

  it('does nothing on the backdrop without onDismiss', async () => {
    await render(
      <Dialog
        actions={[{ label: '知道了', onPress: jest.fn() }]}
        title="提示"
        visible
      />,
    );

    const backdrop = screen.getByTestId('dialog-backdrop');
    expect(backdrop.props.onPress).toBeUndefined();
    await fireEvent.press(backdrop);
    expect(screen.getByText('提示')).toBeOnTheScreen();
  });

  it('dismisses from the backdrop when onDismiss is provided', async () => {
    const onDismiss = jest.fn();
    await render(
      <Dialog
        actions={[{ label: '知道了', onPress: jest.fn() }]}
        onDismiss={onDismiss}
        title="提示"
        visible
      />,
    );

    const backdrop = screen.getByTestId('dialog-backdrop');
    expect(backdrop).toHaveProp('accessibilityLabel', '关闭');
    await fireEvent.press(backdrop);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});

type KeyboardListener = (event: KeyboardEvent) => void;

function captureKeyboard() {
  const listeners = new Map<string, KeyboardListener[]>();
  jest.spyOn(Keyboard, 'addListener').mockImplementation(((
    name: string,
    listener: KeyboardListener,
  ) => {
    listeners.set(name, [...(listeners.get(name) ?? []), listener]);
    return {
      remove: () =>
        listeners.set(
          name,
          (listeners.get(name) ?? []).filter(other => other !== listener),
        ),
    };
  }) as unknown as typeof Keyboard.addListener);
  return {
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

function flat(testID: string) {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
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
  return <TextInput testID="rename-input" />;
}

function renameDialog(visible = true) {
  return (
    <Dialog
      actions={[
        { label: '保存', onPress: jest.fn() },
        { label: '取消', onPress: jest.fn(), variant: 'text' },
      ]}
      message="100.64.0.8"
      testID="rename"
      title="重命名"
      visible={visible}
    >
      <Field />
    </Dialog>
  );
}

describe('Dialog with the software keyboard', () => {
  beforeEach(() => {
    fieldMounts.mockClear();
    jest.replaceProperty(Platform, 'OS', 'ios');
    mockWindowDimensions({ width: 390, height: 844 });
    jest
      .spyOn(LayoutAnimation, 'configureNext')
      .mockImplementation(() => undefined);
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('centres the card in the window above the iOS keyboard and restores it after', async () => {
    const keyboard = captureKeyboard();
    await render(renameDialog());
    expect(flat('rename-overlay')).toMatchObject({
      flex: 1,
      justifyContent: 'center',
      paddingBottom: 0,
    });
    const card = flat('rename-card');

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(544, 300));

    expect(flat('rename-overlay')).toMatchObject({
      justifyContent: 'center',
      paddingBottom: 300,
    });
    expect(flat('rename-card').maxHeight).toBe(844 - 300 - 2 * 16);
    expect(LayoutAnimation.configureNext).toHaveBeenCalledWith({
      duration: 250,
      update: { duration: 250, type: 'keyboard' },
    });

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(844, 300));
    await keyboard.emit('keyboardWillHide', iosKeyboard(844, 300));

    expect(flat('rename-overlay').paddingBottom).toBe(0);
    expect(flat('rename-card')).toEqual(card);
  });

  it('follows keyboardDidShow and keyboardDidHide on Android', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const keyboard = captureKeyboard();
    await render(renameDialog());
    expect(keyboard.events()).toEqual(['keyboardDidHide', 'keyboardDidShow']);

    await keyboard.emit('keyboardDidShow', androidKeyboard(544, 300 - 48));
    expect(flat('rename-overlay').paddingBottom).toBe(300);

    await keyboard.emit('keyboardDidHide', androidKeyboard(844, 0));
    expect(flat('rename-overlay').paddingBottom).toBe(0);
  });

  it('ignores a hardware keyboard', async () => {
    const keyboard = captureKeyboard();
    await render(renameDialog());

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(844, 300));

    expect(flat('rename-overlay').paddingBottom).toBe(0);
  });

  it('listens to the keyboard only while visible', async () => {
    const keyboard = captureKeyboard();
    const view = await render(renameDialog(false));
    expect(keyboard.events()).toEqual([]);

    await view.rerender(renameDialog());
    expect(keyboard.events()).toEqual([
      'keyboardWillChangeFrame',
      'keyboardWillHide',
    ]);

    await view.rerender(renameDialog(false));
    expect(keyboard.events()).toEqual([]);
  });

  it('scrolls the message and field between the pinned title and buttons', async () => {
    const keyboard = captureKeyboard();
    await render(renameDialog());
    await layout('rename-actions', 100);
    await fireEvent(screen.getByRole('header'), 'layout', {
      nativeEvent: { layout: { x: 0, y: 0, width: 350, height: 26 } },
    });

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(508, 336));

    const cardMaxHeight = 844 - 336 - 2 * 16;
    expect(flat('rename-card').maxHeight).toBe(cardMaxHeight);
    const frame = screen.getByTestId('rename-frame');
    expect(frame).toHaveProp('scrollEnabled', false);
    expect(
      StyleSheet.flatten(frame.props.contentContainerStyle).maxHeight,
    ).toBe(cardMaxHeight - 2 * (20 + 1));
    const body = screen.getByTestId('rename-scroll');
    expect(body).toHaveProp('keyboardShouldPersistTaps', 'handled');
    expect(body).toContainElement(screen.getByText('100.64.0.8'));
    expect(body).toContainElement(screen.getByTestId('rename-input'));
    expect(body).not.toContainElement(screen.getByText('保存'));
    expect(body).not.toContainElement(screen.getByRole('header'));
  });

  it('scrolls the whole card when the buttons leave no room above a landscape keyboard', async () => {
    mockWindowDimensions({ width: 852, height: 393 });
    const keyboard = captureKeyboard();
    await render(renameDialog());
    await layout('rename-actions', 100);
    await fireEvent(screen.getByRole('header'), 'layout', {
      nativeEvent: { layout: { x: 0, y: 0, width: 350, height: 26 } },
    });

    await keyboard.emit('keyboardWillChangeFrame', iosKeyboard(193, 200));

    expect(flat('rename-overlay').paddingBottom).toBe(200);
    expect(flat('rename-card').maxHeight).toBe(393 - 200 - 2 * 16);
    const frame = screen.getByTestId('rename-frame');
    expect(frame).toHaveProp('scrollEnabled', true);
    expect(frame).toHaveProp('keyboardShouldPersistTaps', 'handled');
    expect(
      StyleSheet.flatten(frame.props.contentContainerStyle).maxHeight,
    ).toBeUndefined();
    expect(frame).toContainElement(screen.getByRole('header'));
    expect(frame).toContainElement(screen.getByTestId('rename-input'));
    expect(frame).toContainElement(screen.getByText('保存'));
    expect(fieldMounts).toHaveBeenCalledTimes(1);
  });

  it('keeps a message-less confirmation to a title and buttons', async () => {
    await render(
      <Dialog
        actions={[{ label: '知道了', onPress: jest.fn() }]}
        testID="plain"
        title="提示"
        visible
      />,
    );

    expect(screen.queryByTestId('plain-scroll')).toBeNull();
    expect(screen.getByTestId('plain-frame')).toContainElement(
      screen.getByText('知道了'),
    );
  });
});

describe('Dialog with a loading action', () => {
  function removeDialog(
    loading: boolean,
    handlers = { confirm: jest.fn(), cancel: jest.fn(), dismiss: jest.fn() },
  ) {
    return (
      <Dialog
        actions={[
          {
            label: '删除',
            loading,
            onPress: handlers.confirm,
            testID: 'remove-confirm',
            variant: 'pillDanger',
          },
          {
            label: '取消',
            onPress: handlers.cancel,
            testID: 'remove-cancel',
            variant: 'text',
          },
        ]}
        onDismiss={handlers.dismiss}
        testID="remove"
        title="删除这台电脑？"
        visible
      />
    );
  }

  it('holds every button and cannot be dismissed while loading', async () => {
    const handlers = {
      confirm: jest.fn(),
      cancel: jest.fn(),
      dismiss: jest.fn(),
    };
    await render(removeDialog(true, handlers));

    const confirm = screen.getByTestId('remove-confirm');
    expect(confirm).toBeBusy();
    expect(screen.getByTestId('remove-confirm-spinner')).toBeOnTheScreen();
    const cancel = screen.getByTestId('remove-cancel');
    expect(cancel).toBeDisabled();
    await fireEvent.press(confirm);
    await fireEvent.press(cancel);
    expect(handlers.confirm).not.toHaveBeenCalled();
    expect(handlers.cancel).not.toHaveBeenCalled();

    const backdrop = screen.getByTestId('remove-backdrop');
    expect(backdrop.props.onPress).toBeUndefined();
    expect(backdrop.props.accessibilityLabel).toBeUndefined();
    await fireEvent.press(backdrop);
    await fireEvent(screen.getByTestId('remove'), 'requestClose');
    expect(
      screen.getByTestId('remove-overlay').props.onAccessibilityEscape,
    ).toBeUndefined();
    expect(handlers.dismiss).not.toHaveBeenCalled();
    expect(screen.getByText('删除这台电脑？')).toBeOnTheScreen();
  });

  it('releases the buttons and dismissal once loading ends', async () => {
    const handlers = {
      confirm: jest.fn(),
      cancel: jest.fn(),
      dismiss: jest.fn(),
    };
    const view = await render(removeDialog(true, handlers));
    await view.rerender(removeDialog(false, handlers));

    const confirm = screen.getByTestId('remove-confirm');
    expect(confirm).not.toBeBusy();
    expect(confirm).toBeEnabled();
    expect(screen.getByTestId('remove-cancel')).toBeEnabled();
    await fireEvent.press(screen.getByText('取消'));
    expect(handlers.cancel).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByTestId('remove-backdrop'));
    await fireEvent(screen.getByTestId('remove'), 'requestClose');
    await fireEvent(
      screen.getByTestId('remove-overlay'),
      'accessibilityEscape',
    );
    expect(handlers.dismiss).toHaveBeenCalledTimes(3);
  });

  it('disables a single action without holding the others', async () => {
    const save = jest.fn();
    const cancel = jest.fn();
    const onDismiss = jest.fn();
    await render(
      <Dialog
        actions={[
          { label: '保存', onPress: save, disabled: true, testID: 'save' },
          { label: '取消', onPress: cancel, variant: 'text' },
        ]}
        onDismiss={onDismiss}
        title="重命名"
        visible
      />,
    );

    expect(screen.getByTestId('save')).toBeDisabled();
    await fireEvent.press(screen.getByTestId('save'));
    expect(save).not.toHaveBeenCalled();
    await fireEvent.press(screen.getByText('取消'));
    expect(cancel).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByTestId('dialog-backdrop'));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
