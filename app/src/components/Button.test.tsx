import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { PixelRatio, Platform, StyleSheet, processColor } from 'react-native';
import { renderWithProviders } from '../test/render';
import { ripple } from '../theme/platform';
import { Button, type ButtonVariant } from './Button';

const variants: ButtonVariant[] = [
  'primaryLarge',
  'primary',
  'secondary',
  'pill',
  'pillDanger',
  'text',
];

function buttonStyle(testID = 'button') {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
}

function textStyle(label: string) {
  return StyleSheet.flatten(screen.getByText(label).props.style);
}

describe('Button', () => {
  it.each(variants)('renders the title for the %s variant', async variant => {
    await render(
      <Button title="确认" onPress={() => {}} variant={variant} testID="btn" />,
    );
    expect(screen.getByText('确认')).toBeOnTheScreen();
  });

  it('draws the default primary variant with the canvas colors', async () => {
    await render(<Button title="确认" onPress={() => {}} testID="btn" />);
    expect(buttonStyle('btn')).toMatchObject({
      height: 44,
      borderRadius: 12,
      backgroundColor: '#7DA8FF',
      paddingHorizontal: 16,
    });
    expect(textStyle('确认')).toMatchObject({
      color: '#0A1630',
      fontSize: 14,
      fontFamily: 'Geist-SemiBold',
    });
  });

  it('draws primaryLarge at 54 high with a 16 radius', async () => {
    await render(
      <Button
        title="开始连接"
        onPress={() => {}}
        variant="primaryLarge"
        testID="btn"
      />,
    );
    expect(buttonStyle('btn')).toMatchObject({
      height: 54,
      borderRadius: 16,
      paddingHorizontal: 24,
    });
    expect(textStyle('开始连接')).toMatchObject({
      fontSize: 17,
      fontFamily: 'Geist-SemiBold',
      color: '#0A1630',
    });
  });

  it('draws secondary, pill and pillDanger from tokens', async () => {
    await render(
      <Button
        title="次要"
        onPress={() => {}}
        variant="secondary"
        testID="sec"
      />,
    );
    expect(buttonStyle('sec')).toMatchObject({
      height: 44,
      borderRadius: 12,
      backgroundColor: '#1C2328',
    });
    expect(textStyle('次要')).toMatchObject({
      color: '#ECF0F2',
      fontFamily: 'Geist-Regular',
    });

    await render(
      <Button title="胶囊" onPress={() => {}} variant="pill" testID="pill" />,
    );
    expect(buttonStyle('pill')).toMatchObject({
      borderRadius: 22,
      backgroundColor: 'rgba(125, 168, 255, 0.16)',
    });
    expect(textStyle('胶囊')).toMatchObject({ color: '#7DA8FF' });

    await render(
      <Button
        title="断开"
        onPress={() => {}}
        variant="pillDanger"
        testID="danger"
      />,
    );
    expect(buttonStyle('danger')).toMatchObject({
      borderRadius: 22,
      backgroundColor: 'rgba(255, 136, 115, 0.16)',
    });
    expect(textStyle('断开')).toMatchObject({
      color: '#FF8873',
      fontFamily: 'Geist-SemiBold',
    });
  });

  it('draws the text variant without a background', async () => {
    await render(
      <Button title="稍后" onPress={() => {}} variant="text" testID="link" />,
    );
    expect(buttonStyle('link')).toMatchObject({
      height: 44,
      backgroundColor: 'transparent',
    });
    expect(textStyle('稍后')).toMatchObject({
      color: '#7DA8FF',
      fontSize: 15,
      fontFamily: 'Geist-Medium',
    });
  });

  it('calls onPress once', async () => {
    const onPress = jest.fn();
    await render(<Button title="确认" onPress={onPress} testID="btn" />);
    await fireEvent.press(screen.getByTestId('btn'));
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('dims while pressed', async () => {
    jest.useFakeTimers();
    try {
      await render(<Button title="确认" onPress={() => {}} testID="btn" />);
      const button = screen.getByTestId('btn');

      await fireEvent(button, 'responderGrant', {
        persist: () => {},
        nativeEvent: {},
        currentTarget: 1,
      });
      expect(buttonStyle('btn').opacity).toBe(0.8);
      await fireEvent(button, 'responderRelease', {
        persist: () => {},
        nativeEvent: {},
        currentTarget: 1,
      });
      await act(async () => {
        jest.advanceTimersByTime(200);
      });
      expect(buttonStyle('btn').opacity).toBeUndefined();
    } finally {
      jest.useRealTimers();
    }
  });

  it('is disabled: dimmed, announced and silent', async () => {
    const onPress = jest.fn();
    await render(
      <Button title="确认" onPress={onPress} disabled testID="btn" />,
    );
    const button = screen.getByTestId('btn');
    expect(button.props.accessibilityState.disabled).toBe(true);
    expect(buttonStyle('btn').opacity).toBe(0.4);

    await fireEvent.press(button);
    expect(onPress).not.toHaveBeenCalled();
  });

  it('replaces the title with a spinner while loading', async () => {
    const onPress = jest.fn();
    await render(
      <Button title="确认" onPress={onPress} loading testID="btn" />,
    );
    const button = screen.getByTestId('btn');
    expect(screen.queryByText('确认')).toBeNull();
    expect(screen.getByTestId('btn-spinner')).toBeOnTheScreen();
    expect(button.props.accessibilityState.busy).toBe(true);
    expect(button).toBeBusy();

    await fireEvent.press(button);
    expect(onPress).not.toHaveBeenCalled();
  });

  it('renders an 18 px icon in the text color', async () => {
    await render(
      <Button title="添加" onPress={() => {}} icon="plus" testID="btn" />,
    );
    const icon = screen.getByTestId('btn-icon', {
      includeHiddenElements: true,
    });
    expect(icon.props.bbWidth).toBe(18);
    expect(icon.props.bbHeight).toBe(18);
    expect(
      icon.queryAll(node => node.type === 'RNSVGGroup')[0].props.stroke,
    ).toEqual({ type: 0, payload: processColor('#0A1630') });
  });

  it('defaults accessibilityLabel to the title and lets callers override it', async () => {
    const { rerender } = await render(
      <Button title="确认" onPress={() => {}} testID="btn" />,
    );
    expect(screen.getByLabelText('确认')).toBeOnTheScreen();

    await rerender(
      <Button
        title="确认"
        onPress={() => {}}
        accessibilityLabel="确认并继续"
        accessibilityHint="轻点两下继续"
        testID="btn"
      />,
    );
    expect(screen.getByLabelText('确认并继续')).toHaveProp(
      'accessibilityHint',
      '轻点两下继续',
    );
  });

  it('leaves alignment to the parent unless fullWidth or align is set', async () => {
    // No default alignSelf: inside a centred EmptyState the button centres.
    await render(<Button title="确认" onPress={() => {}} testID="auto" />);
    expect(buttonStyle('auto').alignSelf).toBeUndefined();

    await render(
      <Button title="确认" onPress={() => {}} fullWidth testID="full" />,
    );
    expect(buttonStyle('full').alignSelf).toBe('stretch');
  });

  it.each([
    ['start', 'flex-start'],
    ['center', 'center'],
    ['end', 'flex-end'],
    ['stretch', 'stretch'],
  ] as const)('maps align=%s to alignSelf %s', async (align, alignSelf) => {
    await render(
      <Button title="确认" onPress={() => {}} align={align} testID="btn" />,
    );
    expect(buttonStyle('btn').alignSelf).toBe(alignSelf);
  });

  it('lets fullWidth win over align', async () => {
    await render(
      <Button
        title="确认"
        onPress={() => {}}
        align="start"
        fullWidth
        testID="btn"
      />,
    );
    expect(buttonStyle('btn').alignSelf).toBe('stretch');
  });
});

describe('Button platform and font scaling', () => {
  afterEach(() => {
    jest.restoreAllMocks();
    jest.useRealTimers();
  });

  it.each(variants)(
    'keeps the Android %s variant at least 48 dp tall',
    async variant => {
      jest.replaceProperty(Platform, 'OS', 'android');
      await renderWithProviders(
        <Button
          title="确认"
          onPress={() => {}}
          variant={variant}
          testID="btn"
        />,
      );
      expect(buttonStyle('btn')).toMatchObject({
        height: variant === 'primaryLarge' ? 54 : 48,
        minWidth: 48,
      });
    },
  );

  it.each(variants)(
    'provides the Android foreground ripple for %s',
    async variant => {
      jest.replaceProperty(Platform, 'OS', 'android');
      const color =
        variant === 'pill'
          ? ripple.accent
          : variant === 'pillDanger'
          ? ripple.danger
          : ripple.neutral;
      await renderWithProviders(
        <Button
          title="确认"
          onPress={() => {}}
          variant={variant}
          testID="btn"
        />,
      );
      expect(
        screen.getByTestId('btn').props.nativeForegroundAndroid,
      ).toMatchObject({
        type: 'RippleAndroid',
        color: processColor(color),
      });
    },
  );

  it('keeps native ripple feedback absent on iOS', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    await renderWithProviders(
      <Button title="确认" onPress={() => {}} testID="btn" />,
    );
    expect(
      screen.getByTestId('btn').props.nativeForegroundAndroid,
    ).toBeUndefined();
    expect(
      screen.getByTestId('btn').props.nativeBackgroundAndroid,
    ).toBeUndefined();
  });

  it('does not dim the Android button while pressed', async () => {
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
        <Button title="确认" onPress={() => {}} testID="btn" />,
      );
      const button = screen.getByTestId('btn');
      await fireEvent(button, 'responderGrant', {
        persist: () => {},
        nativeEvent: {},
        currentTarget: 1,
      });
      expect(buttonStyle('btn').opacity).toBeUndefined();
      expect(commands.setPressed).toHaveBeenCalledWith(expect.anything(), true);
      await fireEvent(button, 'responderRelease', {
        persist: () => {},
        nativeEvent: {},
        currentTarget: 1,
      });
      await act(async () => {
        jest.advanceTimersByTime(200);
      });
      expect(commands.setPressed).toHaveBeenCalledWith(expect.anything(), false);
    } finally {
      nativeView.Commands = previousCommands;
    }
  });

  it.each([
    ['ios', 'primary', 44],
    ['android', 'primary', 48],
    ['ios', 'primaryLarge', 54],
    ['android', 'primaryLarge', 54],
  ] as const)(
    'allows %s %s text to grow above scale 2',
    async (os, variant, minHeight) => {
      jest.replaceProperty(Platform, 'OS', os);
      jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
      await renderWithProviders(
        <Button
          title="确认并继续"
          onPress={() => {}}
          variant={variant}
          testID="btn"
        />,
      );
      expect(buttonStyle('btn').minHeight).toBe(minHeight);
      expect(buttonStyle('btn')).not.toHaveProperty('height');
      expect(
        screen.getByText('确认并继续').props.numberOfLines,
      ).toBeUndefined();
    },
  );

  it('reads platform and font scale again when rendering', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    const getFontScale = jest
      .spyOn(PixelRatio, 'getFontScale')
      .mockReturnValue(2);
    const button = <Button title="确认" onPress={() => {}} testID="btn" />;
    const { rerender } = await renderWithProviders(button);
    expect(buttonStyle('btn').height).toBe(44);

    jest.replaceProperty(Platform, 'OS', 'android');
    getFontScale.mockReturnValue(3);
    await rerender(<Button title="确认" onPress={() => {}} testID="btn" />);
    expect(buttonStyle('btn').minHeight).toBe(48);
    expect(buttonStyle('btn')).not.toHaveProperty('height');

    jest.replaceProperty(Platform, 'OS', 'ios');
    getFontScale.mockReturnValue(2);
    await rerender(<Button title="确认" onPress={() => {}} testID="btn" />);
    expect(buttonStyle('btn').height).toBe(44);
    expect(buttonStyle('btn')).not.toHaveProperty('minHeight');
  });
});
