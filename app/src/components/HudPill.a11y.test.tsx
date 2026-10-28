import { cleanup, fireEvent, screen } from '@testing-library/react-native';
import { PixelRatio, Platform, processColor, StyleSheet } from 'react-native';
import { renderWithProviders } from '../test/render';
import { hit } from '../theme/tokens';
import { HudPill, type HudPillMode } from './HudPill';

// The iOS preset omits Android ripple commands; keep Pressable's JS state real.
jest.mock('react-native/Libraries/Components/View/ViewNativeComponent', () => ({
  ...jest.requireActual('@react-native/jest-preset/jest/mocks/ViewNativeComponent'),
  __esModule: true,
  Commands: { hotspotUpdate: jest.fn(), setPressed: jest.fn() },
}));

async function mount(mode: HudPillMode = 'expanded') {
  return renderWithProviders(
    <HudPill
      accessibilityLabel="打开画质面板"
      alertText="已降到 1080p30"
      mode={mode}
      onPress={jest.fn()}
      path="tailnetDirect"
      testID="pill"
    />,
  );
}

function pillStyle() {
  return StyleSheet.flatten(screen.getByTestId('pill').props.style);
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('HudPill touch targets, font scaling and platform feedback', () => {
  it.each(
    (['ios', 'android'] as const).flatMap(os =>
      (['expanded', 'alert', 'collapsed'] as const).map(mode => ({ os, mode })),
    ),
  )('expands the $mode hit area using hit.min on $os', async ({ os, mode }) => {
    jest.replaceProperty(Platform, 'OS', os);
    await mount(mode);
    const slop = Math.max(0, (hit.min - 32) / 2);
    expect(screen.getByTestId('pill')).toHaveProp(
      'hitSlop',
      mode === 'collapsed'
        ? { top: slop, bottom: slop, left: slop, right: slop }
        : { top: slop, bottom: slop },
    );
  });

  it.each(['expanded', 'alert'] as const)(
    'lets the %s pill grow at font scale 3',
    async mode => {
      jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
      await mount(mode);
      expect(pillStyle().minHeight).toBe(32);
      expect(pillStyle().height).toBeUndefined();
      const text = screen.getByText(mode === 'alert' ? '已降到 1080p30' : '直连');
      expect(text.props.numberOfLines).toBeUndefined();
    },
  );

  it('keeps the icon-only collapsed pill at 32 by 32 at font scale 3', async () => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    await mount('collapsed');
    expect(pillStyle()).toMatchObject({ width: 32, height: 32, borderRadius: 16 });
  });

  it.each(['ios', 'android'] as const)(
    'uses ripple on Android and opacity on iOS while pressed (%s)',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await mount();
      const pill = screen.getByTestId('pill');
      // Pressable converts android_ripple into the native foreground prop.
      if (os === 'android') {
        expect(pill).toHaveProp(
          'nativeForegroundAndroid',
          expect.objectContaining({
            type: 'RippleAndroid',
            color: processColor('rgba(255, 255, 255, 0.12)'),
          }),
        );
        expect(pillStyle().overflow).toBe('hidden');
      } else {
        expect(pill.props.nativeForegroundAndroid).toBeUndefined();
        expect(pill.props.nativeBackgroundAndroid).toBeUndefined();
        expect(pillStyle().overflow).toBeUndefined();
      }
      const event = {
        persist: jest.fn(),
        currentTarget: { measure: jest.fn() },
        nativeEvent: { locationX: 0, locationY: 0, pageX: 0, pageY: 0 },
      };
      await fireEvent(pill, 'responderGrant', event);
      expect(pillStyle().opacity).toBe(os === 'ios' ? 0.8 : undefined);
      await fireEvent(pill, 'responderTerminate', event);
    },
  );
});
