import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { PixelRatio, Platform, StyleSheet, processColor } from 'react-native';
import { renderWithProviders } from '../test/render';
import { ripple } from '../theme/platform';
import { Segmented } from './Segmented';

type Mode = 'pad' | 'touch';

const options: { value: Mode; label: string }[] = [
  { value: 'pad', label: '触控板' },
  { value: 'touch', label: '触屏直点' },
];

function optionStyle(label: string) {
  return StyleSheet.flatten(screen.getByLabelText(label).props.style);
}

const touch = { persist: () => {}, nativeEvent: {}, currentTarget: 1 };

async function pressIn(label: string) {
  await fireEvent(screen.getByLabelText(label), 'responderGrant', touch);
}

async function pressOut(label: string) {
  await fireEvent(screen.getByLabelText(label), 'responderRelease', touch);
  await act(async () => {
    jest.advanceTimersByTime(200);
  });
}

describe('Segmented', () => {
  it('calls onChange with the tapped option', async () => {
    const onChange = jest.fn();
    await render(
      <Segmented
        options={options}
        value="pad"
        onChange={onChange}
        accessibilityLabel="触控模式"
        testID="mode"
      />,
    );
    await fireEvent.press(screen.getByLabelText('触屏直点'));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith('touch');
  });

  it('does not call onChange for the already selected option', async () => {
    const onChange = jest.fn();
    await render(
      <Segmented
        options={options}
        value="pad"
        onChange={onChange}
        accessibilityLabel="触控模式"
        testID="mode"
      />,
    );
    await fireEvent.press(screen.getByLabelText('触控板'));
    expect(onChange).not.toHaveBeenCalled();
  });

  it('exposes a radiogroup with radio children', async () => {
    await render(
      <Segmented
        options={options}
        value="pad"
        onChange={() => {}}
        accessibilityLabel="触控模式"
        testID="mode"
      />,
    );
    expect(screen.getByLabelText('触控模式')).toHaveProp(
      'accessibilityRole',
      'radiogroup',
    );
    const pad = screen.getByLabelText('触控板');
    expect(pad).toHaveProp('accessibilityRole', 'radio');
    expect(pad.props.accessibilityState.selected).toBe(true);
    expect(pad).toBeSelected();
    expect(
      screen.getByLabelText('触屏直点').props.accessibilityState.selected,
    ).toBe(false);
  });

  it('prefers the per-option accessibilityLabel', async () => {
    await render(
      <Segmented
        options={[
          { value: 'pad', label: '触控板', accessibilityLabel: '触控板模式' },
          { value: 'touch', label: '触屏直点' },
        ]}
        value="touch"
        onChange={() => {}}
        accessibilityLabel="触控模式"
      />,
    );
    expect(screen.getByLabelText('触控板模式')).toBeOnTheScreen();
  });

  it('draws the page variant from the Settings canvas', async () => {
    await render(
      <Segmented
        options={options}
        value="pad"
        onChange={() => {}}
        accessibilityLabel="触控模式"
        testID="mode"
      />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('mode').props.style),
    ).toMatchObject({
      backgroundColor: '#0D1114',
      borderRadius: 12,
      padding: 4,
      gap: 4,
    });
    expect(optionStyle('触控板')).toMatchObject({
      height: 40,
      borderRadius: 9,
      backgroundColor: '#2E3840',
      flex: 1,
    });
    expect(optionStyle('触屏直点').backgroundColor).toBeUndefined();
    expect(
      StyleSheet.flatten(screen.getByText('触控板').props.style),
    ).toMatchObject({ color: '#ECF0F2', fontSize: 14 });
    expect(
      StyleSheet.flatten(screen.getByText('触屏直点').props.style),
    ).toMatchObject({ color: '#A9B4BB' });
  });

  it('draws the overlay variant with a white selected pill', async () => {
    await render(
      <Segmented
        options={options}
        value="touch"
        onChange={() => {}}
        variant="overlay"
        accessibilityLabel="触控模式"
        testID="mode"
      />,
    );
    expect(optionStyle('触屏直点')).toMatchObject({
      height: 40,
      borderRadius: 20,
      paddingHorizontal: 12,
      backgroundColor: '#FFFFFF',
    });
    expect(
      StyleSheet.flatten(screen.getByText('触屏直点').props.style),
    ).toMatchObject({ color: '#0D1114', fontSize: 13 });
    expect(optionStyle('触控板').backgroundColor).toBeUndefined();
    expect(optionStyle('触控板').flex).toBeUndefined();
  });
});

describe('Segmented pressed and disabled options', () => {
  afterEach(() => jest.restoreAllMocks());

  it.each(['page', 'overlay'] as const)(
    'highlights an unselected %s option while pressed on iOS',
    async variant => {
      jest.replaceProperty(Platform, 'OS', 'ios');
      jest.useFakeTimers();
      try {
        await render(
          <Segmented
            options={options}
            value="pad"
            onChange={() => {}}
            variant={variant}
            accessibilityLabel="触控模式"
          />,
        );
        await pressIn('触屏直点');
        expect(optionStyle('触屏直点').backgroundColor).toBe(
          'rgba(255, 255, 255, 0.08)',
        );
        await pressOut('触屏直点');
        expect(optionStyle('触屏直点').backgroundColor).toBeUndefined();
      } finally {
        jest.useRealTimers();
      }
    },
  );

  it('keeps the selected fill while the selected option is pressed', async () => {
    jest.useFakeTimers();
    try {
      await render(
        <Segmented
          options={options}
          value="pad"
          onChange={() => {}}
          accessibilityLabel="触控模式"
        />,
      );
      await pressIn('触控板');
      expect(optionStyle('触控板').backgroundColor).toBe('#2E3840');
      await pressOut('触控板');
    } finally {
      jest.useRealTimers();
    }
  });

  it('keeps the Android ripple instead of the highlight', async () => {
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
        <Segmented
          options={options}
          value="pad"
          onChange={() => {}}
          accessibilityLabel="触控模式"
        />,
      );
      await pressIn('触屏直点');
      expect(optionStyle('触屏直点').backgroundColor).toBeUndefined();
      expect(commands.setPressed).toHaveBeenCalledWith(expect.anything(), true);
      await pressOut('触屏直点');
    } finally {
      nativeView.Commands = previousCommands;
      jest.useRealTimers();
    }
  });

  it('dims, announces and ignores a disabled option', async () => {
    const onChange = jest.fn();
    await render(
      <Segmented
        options={[
          { value: 'pad', label: '触控板' },
          { value: 'touch', label: '触屏直点', disabled: true },
        ]}
        value="pad"
        onChange={onChange}
        accessibilityLabel="触控模式"
      />,
    );
    const touchOption = screen.getByLabelText('触屏直点');
    expect(optionStyle('触屏直点').opacity).toBe(0.4);
    expect(touchOption.props.accessibilityState).toEqual({
      selected: false,
      disabled: true,
    });
    await fireEvent.press(touchOption);
    expect(onChange).not.toHaveBeenCalled();

    expect(optionStyle('触控板').opacity).toBeUndefined();
    expect(screen.getByLabelText('触控板').props.accessibilityState).toEqual({
      selected: true,
    });
  });
});

describe('Segmented platform and font scaling', () => {
  afterEach(() => jest.restoreAllMocks());

  it.each(['page', 'overlay'] as const)(
    'keeps the Android %s option hitSlop inside a 48 dp parent',
    async variant => {
      jest.replaceProperty(Platform, 'OS', 'android');
      await renderWithProviders(
        <Segmented
          options={options}
          value="pad"
          onChange={() => {}}
          variant={variant}
          accessibilityLabel="触控模式"
          testID="mode"
        />,
      );
      expect(screen.getByLabelText('触控板')).toHaveProp('hitSlop', {
        top: 4,
        bottom: 4,
      });
      expect(
        StyleSheet.flatten(screen.getByTestId('mode').props.style).minHeight,
      ).toBe(48);
      expect(optionStyle('触控板')).toMatchObject({ height: 40, minWidth: 48 });
      expect(
        screen.getByLabelText('触控板').props.nativeBackgroundAndroid,
      ).toMatchObject({
        type: 'RippleAndroid',
        color: processColor(ripple.neutral),
      });
      expect(screen.getByLabelText('触控板').props.accessibilityState).toEqual({
        selected: true,
      });
      expect(
        screen.getByLabelText('触屏直点').props.accessibilityState,
      ).toEqual({ selected: false });
    },
  );

  it('preserves the iOS hitSlop and feedback', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    await renderWithProviders(
      <Segmented
        options={options}
        value="pad"
        onChange={() => {}}
        accessibilityLabel="触控模式"
        testID="mode"
      />,
    );
    expect(screen.getByLabelText('触控板')).toHaveProp('hitSlop', {
      top: 2,
      bottom: 2,
    });
    expect(
      screen.getByLabelText('触控板').props.nativeBackgroundAndroid,
    ).toBeUndefined();
    expect(
      StyleSheet.flatten(screen.getByTestId('mode').props.style).minHeight,
    ).toBeUndefined();
  });

  it.each([
    ['ios', 'page'],
    ['ios', 'overlay'],
    ['android', 'page'],
    ['android', 'overlay'],
  ] as const)(
    'allows %s %s text to grow above scale 2',
    async (os, variant) => {
      jest.replaceProperty(Platform, 'OS', os);
      jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
      await renderWithProviders(
        <Segmented
          options={options}
          value="pad"
          onChange={() => {}}
          variant={variant}
          accessibilityLabel="触控模式"
        />,
      );
      for (const { label } of options) {
        expect(optionStyle(label).minHeight).toBe(40);
        expect(optionStyle(label)).not.toHaveProperty('height');
        expect(screen.getByText(label).props.numberOfLines).toBeUndefined();
      }
    },
  );
});
