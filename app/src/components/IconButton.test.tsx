import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { PixelRatio, Platform, StyleSheet, processColor } from 'react-native';
import { renderWithProviders } from '../test/render';
import { ripple } from '../theme/platform';
import { IconButton } from './IconButton';

function buttonStyle(testID: string) {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
}

const touch = { persist: () => {}, nativeEvent: {}, currentTarget: 1 };

async function pressIn(testID: string) {
  await fireEvent(screen.getByTestId(testID), 'responderGrant', touch);
}

async function pressOut(testID: string) {
  await fireEvent(screen.getByTestId(testID), 'responderRelease', touch);
  await act(async () => {
    jest.advanceTimersByTime(200);
  });
}

describe('IconButton', () => {
  it('is reachable by its required accessibility label and fires onPress', async () => {
    const onPress = jest.fn();
    await render(
      <IconButton
        icon="plus"
        accessibilityLabel="添加设备"
        onPress={onPress}
        testID="add"
      />,
    );
    const button = screen.getByLabelText('添加设备');
    expect(button).toHaveProp('accessibilityRole', 'button');

    await fireEvent.press(button);
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('draws a 44 circle on the raised background by default', async () => {
    await render(
      <IconButton
        icon="plus"
        accessibilityLabel="添加设备"
        onPress={() => {}}
        testID="add"
      />,
    );
    expect(buttonStyle('add')).toMatchObject({
      width: 44,
      height: 44,
      borderRadius: 22,
      backgroundColor: '#1C2328',
    });
  });

  it('supports the accentSoft and plain variants', async () => {
    await render(
      <IconButton
        icon="more"
        accessibilityLabel="更多"
        onPress={() => {}}
        variant="accentSoft"
        testID="soft"
      />,
    );
    expect(buttonStyle('soft')).toMatchObject({
      backgroundColor: 'rgba(125, 168, 255, 0.16)',
    });

    await render(
      <IconButton
        icon="close"
        accessibilityLabel="关闭"
        onPress={() => {}}
        variant="plain"
        testID="plain"
      />,
    );
    expect(buttonStyle('plain').backgroundColor).toBeUndefined();
  });

  it('pads the 36 px size with hitSlop to keep a 44 px target', async () => {
    await render(
      <IconButton
        icon="more"
        accessibilityLabel="更多"
        onPress={() => {}}
        size={36}
        testID="small"
      />,
    );
    const button = screen.getByTestId('small');
    expect(buttonStyle('small')).toMatchObject({ width: 36, height: 36 });
    expect(button).toHaveProp('hitSlop', 4);
  });

  it('shows the badge only for a positive count', async () => {
    await render(
      <IconButton
        icon="bell"
        accessibilityLabel="通知"
        onPress={() => {}}
        badge={3}
        testID="bell"
      />,
    );
    expect(screen.getByText('3')).toBeOnTheScreen();
    expect(buttonStyle('bell-badge')).toMatchObject({
      backgroundColor: '#7DA8FF',
      height: 18,
    });
    expect(StyleSheet.flatten(screen.getByText('3').props.style)).toMatchObject(
      { color: '#0A1630', fontSize: 11 },
    );
  });

  it('hides the badge for zero, negative and missing counts', async () => {
    await render(
      <IconButton
        icon="bell"
        accessibilityLabel="通知"
        onPress={() => {}}
        badge={0}
        testID="zero"
      />,
    );
    expect(screen.queryByTestId('zero-badge')).toBeNull();

    await render(
      <IconButton
        icon="bell"
        accessibilityLabel="通知"
        onPress={() => {}}
        badge={-2}
        testID="negative"
      />,
    );
    expect(screen.queryByTestId('negative-badge')).toBeNull();
  });

  it('dims while pressed on iOS', async () => {
    jest.useFakeTimers();
    try {
      await render(
        <IconButton
          icon="close"
          accessibilityLabel="关闭"
          onPress={() => {}}
          variant="plain"
          testID="close"
        />,
      );
      expect(buttonStyle('close').opacity).toBeUndefined();
      await pressIn('close');
      expect(buttonStyle('close').opacity).toBe(0.6);
      await pressOut('close');
      expect(buttonStyle('close').opacity).toBeUndefined();
    } finally {
      jest.useRealTimers();
    }
  });

  it('is announced as disabled and stays silent when disabled', async () => {
    const onPress = jest.fn();
    await render(
      <IconButton
        icon="plus"
        accessibilityLabel="添加设备"
        onPress={onPress}
        disabled
        testID="add"
      />,
    );
    const button = screen.getByTestId('add');
    expect(button.props.accessibilityState.disabled).toBe(true);
    expect(buttonStyle('add').opacity).toBe(0.4);

    await fireEvent.press(button);
    expect(onPress).not.toHaveBeenCalled();
  });
});

describe('IconButton platform and font scaling', () => {
  afterEach(() => jest.restoreAllMocks());

  it.each([
    [44, 2],
    [36, 6],
  ] as const)(
    'pads the Android %s dp circle with %s dp hitSlop',
    async (size, hitSlop) => {
      jest.replaceProperty(Platform, 'OS', 'android');
      await renderWithProviders(
        <IconButton
          icon="plus"
          accessibilityLabel="添加设备"
          onPress={() => {}}
          size={size}
          testID="add"
        />,
      );
      const button = screen.getByTestId('add');
      expect(button).toHaveProp('hitSlop', hitSlop);
      expect(buttonStyle('add')).toMatchObject({
        width: size,
        height: size,
        borderRadius: size / 2,
      });
      expect(button.props.nativeBackgroundAndroid).toMatchObject({
        type: 'RippleAndroid',
        color: processColor(ripple.neutral),
        borderless: true,
        rippleRadius: size / 2 + 4,
      });
    },
  );

  it('keeps the Android ripple instead of dimming while pressed', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    jest.useFakeTimers();
    // The RN preset omits the native commands used by Android ripple feedback.
    const nativeView = jest.requireMock(
      'react-native/Libraries/Components/View/ViewNativeComponent',
    );
    const previousCommands = nativeView.Commands;
    const commands = { hotspotUpdate: jest.fn(), setPressed: jest.fn() };
    nativeView.Commands = commands;
    try {
      await renderWithProviders(
        <IconButton
          icon="close"
          accessibilityLabel="关闭"
          onPress={() => {}}
          testID="close"
        />,
      );
      await pressIn('close');
      expect(buttonStyle('close').opacity).toBeUndefined();
      expect(commands.setPressed).toHaveBeenCalledWith(expect.anything(), true);
      await pressOut('close');
    } finally {
      nativeView.Commands = previousCommands;
      jest.useRealTimers();
    }
  });

  it('does not enable a native ripple on iOS', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    await renderWithProviders(
      <IconButton
        icon="plus"
        accessibilityLabel="添加设备"
        onPress={() => {}}
        testID="add"
      />,
    );
    expect(
      screen.getByTestId('add').props.nativeBackgroundAndroid,
    ).toBeUndefined();
    expect(screen.getByTestId('add').props.hitSlop).toBeUndefined();
  });

  it.each(['ios', 'android'] as const)(
    'allows the %s badge text to grow above scale 2',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
      await renderWithProviders(
        <IconButton
          icon="bell"
          accessibilityLabel="通知"
          onPress={() => {}}
          badge={3}
          testID="bell"
        />,
      );
      expect(buttonStyle('bell-badge').minHeight).toBe(18);
      expect(buttonStyle('bell-badge')).not.toHaveProperty('height');
      expect(screen.getByText('3').props.numberOfLines).toBeUndefined();
    },
  );
});
