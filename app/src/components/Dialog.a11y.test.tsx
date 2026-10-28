import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import { AccessibilityInfo, Platform, StyleSheet } from 'react-native';
import { renderWithProviders } from '../test/render';
import { Dialog } from './Dialog';

let view: Awaited<ReturnType<typeof renderWithProviders>> | undefined;

async function mount(ui: ReactElement) {
  view = await renderWithProviders(ui);
  return view;
}

function dialog(visible = true, onDismiss?: () => void) {
  return (
    <Dialog
      actions={[{ label: '知道了', onPress: jest.fn() }]}
      onDismiss={onDismiss}
      testID="dialog"
      title="操作确认"
      visible={visible}
    />
  );
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  jest.mocked(AccessibilityInfo.sendAccessibilityEvent).mockClear();
});

afterEach(async () => {
  await cleanup();
  view?.queryClient.clear();
  view = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('focuses the title View after opening', async () => {
  const focus = jest.mocked(AccessibilityInfo.sendAccessibilityEvent);
  await mount(dialog());
  const title = screen.getByRole('header', { name: '操作确认' });
  expect(title.type).toBe('View');
  expect(title).toHaveProp('accessible', true);
  expect(focus).not.toHaveBeenCalled();
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(focus).toHaveBeenCalledWith(
    title.unstable_fiber?.return?.stateNode,
    'focus',
  );
  expect(focus).toHaveBeenCalledTimes(1);
});

it('focuses only when visible becomes true and refocuses when reopened', async () => {
  const focus = jest.mocked(AccessibilityInfo.sendAccessibilityEvent);
  const mounted = await mount(dialog(false));
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(focus).not.toHaveBeenCalled();

  await mounted.rerender(dialog());
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(focus).toHaveBeenCalledTimes(1);
  await mounted.rerender(dialog());
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(focus).toHaveBeenCalledTimes(1);

  await mounted.rerender(dialog(false));
  await mounted.rerender(dialog());
  const title = screen.getByRole('header', { name: '操作确认' });
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(focus).toHaveBeenLastCalledWith(
    title.unstable_fiber?.return?.stateNode,
    'focus',
  );
  expect(focus).toHaveBeenCalledTimes(2);
});

it('cancels pending title focus when hidden', async () => {
  const mounted = await mount(dialog());
  await mounted.rerender(dialog(false));
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
});

it('cancels pending title focus on unmount', async () => {
  const mounted = await mount(dialog());
  await mounted.unmount();
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
});

it('dismisses from accessibility escape when onDismiss is provided', async () => {
  const onDismiss = jest.fn();
  await mount(dialog(true, onDismiss));
  await fireEvent(screen.getByTestId('dialog-overlay'), 'accessibilityEscape');
  expect(onDismiss).toHaveBeenCalledTimes(1);
});

it('keeps the dialog open on accessibility escape without onDismiss', async () => {
  await mount(dialog());
  const root = screen.getByTestId('dialog-overlay');
  expect(root.props.onAccessibilityEscape).toBeUndefined();
  await fireEvent(root, 'accessibilityEscape');
  expect(screen.getByRole('header', { name: '操作确认' })).toBeOnTheScreen();
});

it('uses the Android 28 dp corners, 24 dp padding and translucent system bars', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await mount(dialog());
  expect(
    StyleSheet.flatten(screen.getByTestId('dialog-card').props.style),
  ).toMatchObject({ borderRadius: 28, padding: 24 });
  const modal = screen.getByTestId('dialog');
  expect(modal).toHaveProp('statusBarTranslucent', true);
  expect(modal).toHaveProp('navigationBarTranslucent', true);
});

it('preserves iOS corners, padding and system bar props', async () => {
  await mount(dialog());
  expect(
    StyleSheet.flatten(screen.getByTestId('dialog-card').props.style),
  ).toMatchObject({ borderRadius: 22, padding: 20 });
  const modal = screen.getByTestId('dialog');
  expect(modal.props.statusBarTranslucent).toBeUndefined();
  expect(modal.props.navigationBarTranslucent).toBeUndefined();
});
