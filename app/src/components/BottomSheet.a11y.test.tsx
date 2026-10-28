import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import {
  AccessibilityInfo,
  BackHandler,
  Platform,
  StyleSheet,
} from 'react-native';
import { renderWithProviders } from '../test/render';
import AppText from './AppText';
import { BottomSheet } from './BottomSheet';

let view: Awaited<ReturnType<typeof renderWithProviders>> | undefined;

async function mount(ui: ReactElement) {
  view = await renderWithProviders(ui);
  return view;
}

function sheet(onClose = jest.fn()) {
  return (
    <BottomSheet onClose={onClose} testID="sheet" title="电脑详情">
      <AppText>弹层内容</AppText>
    </BottomSheet>
  );
}

function captureBackPress() {
  type Handler = Parameters<typeof BackHandler.addEventListener>[1];
  let handler: Handler | undefined;
  const remove = jest.fn(() => {
    handler = undefined;
  });
  const add = jest
    .spyOn(BackHandler, 'addEventListener')
    .mockImplementation((_event, listener) => {
      handler = listener;
      return { remove };
    });
  return {
    add,
    remove,
    press: () => handler?.({ type: 'hardwareBackPress', timeStamp: 0 }),
  };
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

it.each(['ios', 'android'] as const)(
  'focuses the title View after mounting on %s',
  async platform => {
    jest.replaceProperty(Platform, 'OS', platform);
    const focus = jest.mocked(AccessibilityInfo.sendAccessibilityEvent);
    await mount(sheet());
    const title = screen.getByRole('header', { name: '电脑详情' });
    expect(title.type).toBe('View');
    expect(title).toHaveProp('accessible', true);
    expect(focus).not.toHaveBeenCalled();

    await act(() => jest.advanceTimersByTimeAsync(0));

    // The RN preset attaches the ref to the View mock that owns this host node.
    expect(focus).toHaveBeenCalledWith(
      title.unstable_fiber?.return?.stateNode,
      'focus',
    );
    expect(focus).toHaveBeenCalledTimes(1);
  },
);

it('cancels pending title focus on unmount', async () => {
  const mounted = await mount(sheet());
  await mounted.unmount();
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
});

it('closes from the accessibility escape gesture', async () => {
  const onClose = jest.fn();
  await mount(sheet(onClose));
  await fireEvent(screen.getByTestId('sheet'), 'accessibilityEscape');
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('keeps the backdrop pressable while removing it from the focus order', async () => {
  const onClose = jest.fn();
  await mount(sheet(onClose));
  const backdrop = screen.getByTestId('sheet-backdrop');
  expect(backdrop).toHaveProp('accessible', false);
  expect(backdrop).toHaveProp('importantForAccessibility', 'no');
  expect(backdrop).toHaveProp('accessibilityLabel', '关闭');
  expect(screen.getAllByRole('button', { name: '关闭' })).toHaveLength(1);
  await fireEvent.press(backdrop);
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('consumes an Android hardware back press and calls onClose exactly once', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const back = captureBackPress();
  const onClose = jest.fn();
  await mount(sheet(onClose));
  expect(back.add).toHaveBeenCalledWith(
    'hardwareBackPress',
    expect.any(Function),
  );
  expect(back.press()).toBe(true);
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('removes the Android back listener on unmount so later presses do nothing', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const back = captureBackPress();
  const onClose = jest.fn();
  const mounted = await mount(sheet(onClose));
  await mounted.unmount();
  expect(back.remove).toHaveBeenCalledTimes(1);
  expect(back.press()).toBeUndefined();
  expect(onClose).not.toHaveBeenCalled();
});

it('leaves hardware back handling unchanged on iOS', async () => {
  const back = captureBackPress();
  await mount(sheet());
  expect(back.add).not.toHaveBeenCalled();
});

it('uses the Android handle and a 48 dp close target', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await mount(sheet());
  const handle = screen.getByTestId('sheet-handle', {
    includeHiddenElements: true,
  });
  expect(StyleSheet.flatten(handle.props.style)).toMatchObject({
    width: 32,
    height: 4,
    borderRadius: 2,
  });
  expect(
    StyleSheet.flatten(screen.getByTestId('sheet-close').props.style),
  ).toMatchObject({ width: 48, height: 48 });
});

function lockedSheet(onClose = jest.fn()) {
  return (
    <BottomSheet
      dismissible={false}
      onClose={onClose}
      testID="sheet"
      title="输入密码"
    >
      <AppText>弹层内容</AppText>
    </BottomSheet>
  );
}

it('keeps a non-dismissible sheet open from the backdrop and escape', async () => {
  const onClose = jest.fn();
  await mount(lockedSheet(onClose));
  const backdrop = screen.getByTestId('sheet-backdrop');
  expect(backdrop.props.onPress).toBeUndefined();
  expect(backdrop.props.accessibilityLabel).toBeUndefined();
  expect(backdrop.props.accessibilityRole).toBeUndefined();
  await fireEvent.press(backdrop);
  expect(
    screen.getByTestId('sheet').props.onAccessibilityEscape,
  ).toBeUndefined();
  await fireEvent(screen.getByTestId('sheet'), 'accessibilityEscape');
  expect(onClose).not.toHaveBeenCalled();
  expect(screen.getByRole('header', { name: '输入密码' })).toBeOnTheScreen();
});

it('still closes a non-dismissible sheet from its close button', async () => {
  const onClose = jest.fn();
  await mount(lockedSheet(onClose));
  await fireEvent.press(screen.getByRole('button', { name: '关闭' }));
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('swallows Android back under a non-dismissible sheet without closing it', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const back = captureBackPress();
  const onClose = jest.fn();
  await mount(lockedSheet(onClose));
  expect(back.press()).toBe(true);
  expect(onClose).not.toHaveBeenCalled();
});
