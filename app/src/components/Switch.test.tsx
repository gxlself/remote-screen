import { fireEvent, render, screen } from '@testing-library/react-native';
import { Animated, Platform, StyleSheet } from 'react-native';
import { haptic } from '../native/platform/impl/system';
import { renderWithProviders } from '../test/render';
import { Switch } from './Switch';

jest.mock('../native/platform/impl/system', () => ({
  ...jest.requireActual('../native/platform/impl/system'),
  haptic: jest.fn(),
}));

beforeEach(() => {
  jest.mocked(haptic).mockClear();
});

function trackStyle(testID = 'switch') {
  return StyleSheet.flatten(screen.getByTestId(`${testID}-track`).props.style);
}

function knobStyle(testID = 'switch') {
  return StyleSheet.flatten(screen.getByTestId(`${testID}-knob`).props.style);
}

describe('Switch', () => {
  it('turns on from the off state', async () => {
    const onValueChange = jest.fn();
    await render(
      <Switch
        value={false}
        onValueChange={onValueChange}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    await fireEvent.press(screen.getByTestId('switch'));
    expect(onValueChange).toHaveBeenCalledTimes(1);
    expect(onValueChange).toHaveBeenCalledWith(true);
  });

  it('turns off from the on state', async () => {
    const onValueChange = jest.fn();
    await render(
      <Switch
        value
        onValueChange={onValueChange}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    await fireEvent.press(screen.getByTestId('switch'));
    expect(onValueChange).toHaveBeenCalledWith(false);
  });

  it('is announced as a switch and follows the value', async () => {
    const { rerender } = await render(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    const off = screen.getByLabelText('自动更新');
    expect(off).toHaveProp('accessibilityRole', 'switch');
    expect(off.props.accessibilityState.checked).toBe(false);
    expect(off).not.toBeChecked();

    await rerender(
      <Switch
        value
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    expect(
      screen.getByLabelText('自动更新').props.accessibilityState.checked,
    ).toBe(true);
    expect(screen.getByLabelText('自动更新')).toBeChecked();
  });

  it('draws the canvas track and knob geometry', async () => {
    await render(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    expect(trackStyle()).toMatchObject({
      width: 51,
      height: 31,
      borderRadius: 16,
      backgroundColor: '#3A444B',
    });
    expect(knobStyle()).toMatchObject({
      width: 27,
      height: 27,
      borderRadius: 13.5,
      backgroundColor: '#FFFFFF',
      left: 2,
    });

    await render(
      <Switch
        value
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="on"
      />,
    );
    expect(trackStyle('on').backgroundColor).toBe('#5DD39A');
    expect(knobStyle('on').left).toBe(22);
  });

  it('keeps a 44 px tall touch area', async () => {
    await render(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('switch').props.style).height,
    ).toBe(44);
  });

  it('accepts an onColor override for the on track', async () => {
    await render(
      <Switch
        value
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        onColor="#7DA8FF"
        testID="switch"
      />,
    );
    expect(trackStyle().backgroundColor).toBe('#7DA8FF');
  });

  it('ignores onColor while off', async () => {
    await render(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        onColor="#7DA8FF"
        testID="switch"
      />,
    );
    expect(trackStyle().backgroundColor).toBe('#3A444B');
  });

  it('is disabled: dimmed, announced and silent', async () => {
    const onValueChange = jest.fn();
    await render(
      <Switch
        value={false}
        onValueChange={onValueChange}
        accessibilityLabel="自动更新"
        disabled
        testID="switch"
      />,
    );
    const control = screen.getByTestId('switch');
    expect(control.props.accessibilityState.disabled).toBe(true);
    expect(StyleSheet.flatten(control.props.style).opacity).toBe(0.4);

    await fireEvent.press(control);
    expect(onValueChange).not.toHaveBeenCalled();
  });
});

describe('Switch feedback', () => {
  afterEach(() => jest.restoreAllMocks());

  it('taps a light haptic on every toggle', async () => {
    const onValueChange = jest.fn();
    await render(
      <Switch
        value={false}
        onValueChange={onValueChange}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    await fireEvent.press(screen.getByTestId('switch'));
    expect(haptic).toHaveBeenCalledTimes(1);
    expect(haptic).toHaveBeenCalledWith('light');
    expect(onValueChange).toHaveBeenCalledWith(true);
  });

  it('stays silent and still when disabled', async () => {
    await render(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        disabled
        testID="switch"
      />,
    );
    await fireEvent.press(screen.getByTestId('switch'));
    expect(haptic).not.toHaveBeenCalled();
  });

  it('renders the knob at rest without starting an animation', async () => {
    const timing = jest.spyOn(Animated, 'timing');
    await render(
      <Switch
        value
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    expect(timing).not.toHaveBeenCalled();
    expect(knobStyle()).toMatchObject({
      left: 22,
      transform: [{ translateX: 0 }],
    });
  });

  it.each([
    ['ios', false, 2, 22],
    ['ios', true, 22, 2],
    ['android', false, 8, 24],
    ['android', true, 24, 8],
  ] as const)(
    'slides the %s knob in from where it was when %s flips',
    async (os, from, fromLeft, toLeft) => {
      jest.replaceProperty(Platform, 'OS', os);
      const start = jest.fn();
      const timing = jest
        .spyOn(Animated, 'timing')
        .mockReturnValue({ start, stop: jest.fn(), reset: jest.fn() });
      const { rerender } = await renderWithProviders(
        <Switch
          value={from}
          onValueChange={() => {}}
          accessibilityLabel="自动更新"
          testID="switch"
        />,
      );
      await rerender(
        <Switch
          value={!from}
          onValueChange={() => {}}
          accessibilityLabel="自动更新"
          testID="switch"
        />,
      );
      expect(knobStyle().left).toBe(toLeft);
      expect(timing).toHaveBeenCalledTimes(1);
      const [slide, config] = timing.mock.calls[0];
      expect(config).toMatchObject({
        toValue: 0,
        duration: 160,
        useNativeDriver: true,
      });
      let offset: number | undefined;
      (slide as Animated.Value).stopAnimation(value => {
        offset = value;
      });
      expect(offset).toBe(fromLeft - toLeft);
      expect(start).toHaveBeenCalledTimes(1);
    },
  );
});

describe('Switch platform variants', () => {
  afterEach(() => jest.restoreAllMocks());

  it.each([false, true])(
    'renders the Android M3 geometry with value %s',
    async value => {
      jest.replaceProperty(Platform, 'OS', 'android');
      const onValueChange = jest.fn();
      await renderWithProviders(
        <Switch
          value={value}
          onValueChange={onValueChange}
          accessibilityLabel="自动更新"
          testID="switch"
        />,
      );
      const control = screen.getByLabelText('自动更新');
      expect(StyleSheet.flatten(control.props.style)).toMatchObject({
        width: 52,
        height: 48,
      });
      expect(trackStyle()).toMatchObject({
        width: 52,
        height: 32,
        borderRadius: 16,
        backgroundColor: value ? '#5DD39A' : '#3A444B',
      });
      if (value) {
        expect(trackStyle().borderWidth).toBeUndefined();
      } else {
        expect(trackStyle()).toMatchObject({
          borderWidth: 2,
          borderColor: '#4E5A63',
        });
      }
      expect(knobStyle()).toMatchObject({
        width: value ? 24 : 16,
        height: value ? 24 : 16,
        borderRadius: value ? 12 : 8,
        top: value ? 4 : 8,
        left: value ? 24 : 8,
        backgroundColor: value ? '#FFFFFF' : '#86929A',
      });
      expect(control).toHaveProp('accessibilityRole', 'switch');
      expect(control.props.accessibilityState).toEqual({
        checked: value,
        disabled: false,
      });
      await fireEvent.press(control);
      expect(onValueChange).toHaveBeenCalledWith(!value);
    },
  );

  it('preserves the Android onColor override', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderWithProviders(
      <Switch
        value
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        onColor="#7DA8FF"
        testID="switch"
      />,
    );
    expect(trackStyle().backgroundColor).toBe('#7DA8FF');
  });

  it.each(['ios', 'android'] as const)(
    'forwards the optional accessibility hint on %s',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await renderWithProviders(
        <Switch
          value={false}
          onValueChange={() => {}}
          accessibilityLabel="自动更新"
          accessibilityHint="开启后自动检查更新"
        />,
      );
      expect(screen.getByLabelText('自动更新')).toHaveProp(
        'accessibilityHint',
        '开启后自动检查更新',
      );
    },
  );

  it('reads all dimensions again when the platform changes', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    const { rerender } = await renderWithProviders(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    expect(trackStyle()).toMatchObject({ width: 51, height: 31 });
    jest.replaceProperty(Platform, 'OS', 'android');
    await rerender(
      <Switch
        value={false}
        onValueChange={() => {}}
        accessibilityLabel="自动更新"
        testID="switch"
      />,
    );
    expect(trackStyle()).toMatchObject({ width: 52, height: 32 });
    expect(
      StyleSheet.flatten(screen.getByTestId('switch').props.style).height,
    ).toBe(48);
  });
});
