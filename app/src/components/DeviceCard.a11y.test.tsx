import { fireEvent, screen } from '@testing-library/react-native';
import { Platform, processColor, StyleSheet } from 'react-native';
import { computers } from '../native/rscore/fixtures';
import { renderWithProviders } from '../test/render';
import { DeviceCard } from './DeviceCard';

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
});

afterEach(() => {
  jest.restoreAllMocks();
});

describe('DeviceCard accessible details action and platform feedback', () => {
  it.each(['ios', 'android'] as const)(
    'offers the details action and invokes long press once on %s',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      const onLongPress = jest.fn();
      const onPress = jest.fn();
      await renderWithProviders(
        <DeviceCard
          name={computers[0].name}
          look="live"
          onLongPress={onLongPress}
          onPress={onPress}
          testID="card"
        />,
      );
      const card = screen.getByTestId('card');
      expect(card).toHaveProp('accessibilityActions', [
        { name: 'longpress', label: '查看详情' },
      ]);
      expect(card).toHaveProp('accessibilityHint', '轻点两下开始远控，长按看详情');
      await fireEvent(card, 'accessibilityAction', {
        nativeEvent: { actionName: 'longpress' },
      });
      expect(onLongPress).toHaveBeenCalledTimes(1);
      expect(onPress).not.toHaveBeenCalled();
    },
  );

  it('does not advertise or handle a details action without onLongPress', async () => {
    await renderWithProviders(
      <DeviceCard name={computers[0].name} look="live" onPress={jest.fn()} testID="card" />,
    );
    expect(screen.getByTestId('card').props.accessibilityActions).toBeUndefined();
    expect(screen.getByTestId('card').props.onAccessibilityAction).toBeUndefined();
  });

  it('ignores unrelated accessibility actions', async () => {
    const onLongPress = jest.fn();
    await renderWithProviders(
      <DeviceCard
        name={computers[0].name}
        look="live"
        onLongPress={onLongPress}
        onPress={jest.fn()}
        testID="card"
      />,
    );
    await fireEvent(screen.getByTestId('card'), 'accessibilityAction', {
      nativeEvent: { actionName: 'other' },
    });
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it.each(['ios', 'android'] as const)(
    'clips ripple feedback to the card only on Android (%s)',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await renderWithProviders(
        <DeviceCard name={computers[0].name} look="live" onPress={jest.fn()} testID="card" />,
      );
      const card = screen.getByTestId('card');
      const style = StyleSheet.flatten(card.props.style);
      expect(style.borderRadius).toBe(16);
      // Pressable converts android_ripple into the native foreground prop.
      if (os === 'android') {
        expect(card).toHaveProp(
          'nativeForegroundAndroid',
          expect.objectContaining({
            type: 'RippleAndroid',
            color: processColor('rgba(255, 255, 255, 0.12)'),
          }),
        );
        expect(style.overflow).toBe('hidden');
      } else {
        expect(card.props.nativeForegroundAndroid).toBeUndefined();
        expect(card.props.nativeBackgroundAndroid).toBeUndefined();
        expect(style.overflow).toBeUndefined();
      }
    },
  );
});
