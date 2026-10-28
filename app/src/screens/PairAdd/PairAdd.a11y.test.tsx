import {
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { expectLabels } from '../../test/a11y/helpers';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import PairAddScreen from '.';

const originalPlatform = Platform.OS;

function props(params?: RootScreenProps<'PairAdd'>['route']['params']) {
  return {
    navigation: mockNavigation<'PairAdd'>(),
    route: mockRoute('PairAdd', params),
  } as RootScreenProps<'PairAdd'>;
}

function keyboardBehavior(instance: { unstable_fiber?: unknown }) {
  let fiber = instance.unstable_fiber as
    | { memoizedProps?: Record<string, unknown>; return?: unknown }
    | null
    | undefined;
  while (fiber != null) {
    if (fiber.memoizedProps?.behavior !== undefined) {
      return fiber.memoizedProps.behavior;
    }
    fiber = fiber.return as typeof fiber;
  }
  return undefined;
}

async function renderPairAdd(
  params?: RootScreenProps<'PairAdd'>['route']['params'],
) {
  const view = await renderWithProviders(<PairAddScreen {...props(params)} />, {
    core: createFakeCore(),
    localDirectory: {},
  });
  await act(async () => {
    await Promise.resolve();
  });
  return view;
}

async function openCodeTab() {
  await fireEvent.press(screen.getByLabelText('输入配对码'));
}

beforeEach(() => {
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: 'ios',
  });
  jest.clearAllMocks();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: originalPlatform,
  });
});

describe('PairAdd accessibility', () => {
  it('marks the page title as a header', async () => {
    await renderPairAdd();
    expect(screen.getByText('配对电脑')).toHaveProp(
      'accessibilityRole',
      'header',
    );
  });

  it('hides the code input from Android while keeping the iOS behavior', async () => {
    await renderPairAdd();
    await openCodeTab();
    expect(screen.getByTestId('pair-code-input').props).not.toHaveProperty(
      'importantForAccessibility',
    );

    await cleanup();
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    await renderPairAdd();
    await openCodeTab();
    expect(screen.getByTestId('pair-code-input')).toHaveProp(
      'importantForAccessibility',
      'no',
    );
  });

  it('pads code box hit targets from the shared minimum', async () => {
    await renderPairAdd();
    await openCodeTab();
    const iosBox = screen.getByTestId('pair-code-box-1');
    expect(iosBox.props.hitSlop.left).toBe(Math.max(0, (hit.min - 46) / 2));

    await cleanup();
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    await renderPairAdd();
    await openCodeTab();
    const androidBox = screen.getByTestId('pair-code-box-1');
    expect(androidBox.props.hitSlop.left).toBe(Math.max(0, (hit.min - 46) / 2));
  });

  it('wraps and grows code boxes at a large font scale', async () => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    await renderPairAdd();
    await openCodeTab();
    expect(
      StyleSheet.flatten(screen.getByTestId('pair-code-box-1').props.style),
    ).toMatchObject({
      minHeight: 56,
    });
    expect(
      StyleSheet.flatten(
        screen.getByTestId('pair-code-box-1').parent?.props.style,
      ),
    ).toMatchObject({
      flexWrap: 'wrap',
      rowGap: 7,
    });
    expect(
      StyleSheet.flatten(screen.getByTestId('pair-code-box-1').props.style),
    ).not.toHaveProperty('height');
  });

  it('keeps the header still on iOS and insets only the form scroll', async () => {
    await renderPairAdd();
    await openCodeTab();
    expect(
      keyboardBehavior(screen.getByTestId('pair-add-screen')),
    ).toBeUndefined();
    const scroll = screen.getByTestId('pair-add-screen-scroll');
    expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
    expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
    expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
    expect(scroll).toContainElement(screen.getByTestId('pair-code-input'));
    expect(scroll).toContainElement(screen.getByTestId('pair-submit'));
    expect(scroll).not.toContainElement(screen.getByText('配对电脑'));
  });

  it('uses padding keyboard avoidance on android', async () => {
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    await renderPairAdd();
    expect(keyboardBehavior(screen.getByTestId('pair-add-screen'))).toBe(
      'padding',
    );
  });

  it('labels the address field and every control of the address form', async () => {
    const view = await renderPairAdd({ tab: 'code' });
    const field = screen.getByTestId('pair-addr-input');
    expect(field).toHaveProp('accessibilityLabel', '电脑地址');
    expect(screen.getByLabelText('电脑地址')).toBe(field);
    expectLabels(view.root);
  });
});
