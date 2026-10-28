import { render, screen } from '@testing-library/react-native';
import { KeyboardAvoidingView, Platform, StyleSheet, View } from 'react-native';
import { FormScroll } from './FormScroll';

type Fiber = {
  type?: unknown;
  memoizedProps?: Record<string, unknown>;
  return?: Fiber | null;
};

function keyboardAvoider(instance: { unstable_fiber?: unknown }) {
  let fiber = instance.unstable_fiber as Fiber | null | undefined;
  while (fiber != null) {
    if (fiber.type === KeyboardAvoidingView) {
      return fiber.memoizedProps ?? null;
    }
    fiber = fiber.return;
  }
  return null;
}

function childIDs(testID: string) {
  return screen
    .getByTestId(testID)
    .children.map(child =>
      typeof child === 'string' ? child : child.props.testID,
    );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#000' },
  content: { padding: 20 },
});

async function renderForm(
  importantForAccessibility?: 'auto' | 'no-hide-descendants',
) {
  await render(
    <FormScroll
      contentContainerStyle={styles.content}
      footer={<View testID="form-footer" />}
      header={<View testID="form-header" />}
      importantForAccessibility={importantForAccessibility}
      style={styles.screen}
      testID="form"
    >
      <View testID="form-field" />
    </FormScroll>,
  );
}

afterEach(() => {
  jest.restoreAllMocks();
});

describe('FormScroll', () => {
  it('lets the iOS scroll view inset itself by the keyboard instead of avoiding it', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    await renderForm();

    const scroll = screen.getByTestId('form-scroll');
    expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
    expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
    expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
    expect(scroll).toHaveProp('showsVerticalScrollIndicator', false);
    expect(scroll).not.toHaveProp('contentInsetAdjustmentBehavior');
    expect(keyboardAvoider(scroll)).toBeNull();
    expect(keyboardAvoider(screen.getByTestId('form'))).toBeNull();
    expect(screen.getByTestId('form')).toHaveStyle({
      flex: 1,
      backgroundColor: '#000',
    });
  });

  it('keeps the header and footer outside the iOS scroll view', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    await renderForm();

    expect(childIDs('form')).toEqual([
      'form-header',
      'form-scroll',
      'form-footer',
    ]);
    expect(screen.getByTestId('form-scroll')).toContainElement(
      screen.getByTestId('form-field'),
    );
  });

  it('keeps the padding KeyboardAvoidingView layout on Android', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderForm();

    const outer = screen.getByTestId('form');
    expect(keyboardAvoider(outer)).toMatchObject({
      behavior: 'padding',
      style: styles.screen,
      testID: 'form',
    });
    expect(outer).toHaveStyle({ flex: 1, backgroundColor: '#000' });
    expect(childIDs('form')).toEqual([
      'form-header',
      'form-scroll',
      'form-footer',
    ]);

    const scroll = screen.getByTestId('form-scroll');
    expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
    expect(scroll).toHaveProp('showsVerticalScrollIndicator', false);
    expect(scroll).not.toHaveProp('automaticallyAdjustKeyboardInsets');
    expect(scroll).not.toHaveProp('keyboardDismissMode');
  });

  it.each(['ios', 'android'] as const)(
    'passes the content style and accessibility mode to the %s scroll view',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await renderForm('no-hide-descendants');

      const scroll = screen.getByTestId('form-scroll', {
        includeHiddenElements: true,
      });
      expect(scroll).toHaveProp('contentContainerStyle', styles.content);
      expect(scroll).toHaveProp(
        'importantForAccessibility',
        'no-hide-descendants',
      );
    },
  );
});
