import { cleanup, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import { AccessibilityInfo, Platform, StyleSheet } from 'react-native';
import { mockWindowDimensions } from '../test/dimensions';
import { renderWithProviders } from '../test/render';
import { Toast } from './Toast';

let view: Awaited<ReturnType<typeof renderWithProviders>> | undefined;

async function mount(ui: ReactElement) {
  view = await renderWithProviders(ui);
  return view;
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  jest.mocked(AccessibilityInfo.announceForAccessibility).mockClear();
});

afterEach(async () => {
  await cleanup();
  view?.queryClient.clear();
  view = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('announces each shown iOS message once', async () => {
  const announce = jest.mocked(AccessibilityInfo.announceForAccessibility);
  const mounted = await mount(<Toast onHide={jest.fn()} toast={null} />);
  expect(announce).not.toHaveBeenCalled();
  await mounted.rerender(
    <Toast onHide={jest.fn()} toast={{ message: '已复制' }} />,
  );
  expect(announce).toHaveBeenCalledWith('已复制');
  expect(announce).toHaveBeenCalledTimes(1);
});

it('uses the Android live region without manual or duplicate announcements', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const mounted = await mount(
    <Toast onHide={jest.fn()} toast={{ message: '已复制' }} />,
  );
  const toast = screen.getByTestId('toast');
  expect(toast).toHaveProp('accessibilityLiveRegion', 'polite');
  expect(toast).toHaveProp('accessibilityLabel', '已复制');
  await mounted.rerender(
    <Toast onHide={jest.fn()} toast={{ message: '再返回一次回到设备页' }} />,
  );
  expect(screen.getByTestId('toast')).toHaveProp(
    'accessibilityLabel',
    '再返回一次回到设备页',
  );
  expect(AccessibilityInfo.announceForAccessibility).not.toHaveBeenCalled();
});

it.each([390, 1000])(
  'uses an Android Snackbar with 12 dp side insets at width %i',
  async width => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const restore = mockWindowDimensions({ width, height: 844 });
    try {
      await mount(<Toast onHide={jest.fn()} toast={{ message: '已复制' }} />);
      const toast = screen.getByTestId('toast');
      expect(StyleSheet.flatten(toast.props.style)).toMatchObject({
        borderRadius: 4,
        minHeight: 48,
        width: '100%',
        maxWidth: 560,
        backgroundColor: 'rgba(10, 13, 16, 0.8)',
        borderColor: 'rgba(255, 255, 255, 0.08)',
      });
      // The layer spans its container rather than the window, so a toast in a
      // padded or narrower container stays centred in it.
      const layer = StyleSheet.flatten(toast.parent?.props.style);
      expect(layer).toMatchObject({
        left: 0,
        right: 0,
        paddingHorizontal: 12,
        alignItems: 'center',
        bottom: 100,
      });
      expect(layer).not.toHaveProperty('width');
      expect(
        StyleSheet.flatten(screen.getByText('已复制').props.style),
      ).toMatchObject({ textAlign: 'left' });
    } finally {
      restore();
    }
  },
);

it.each(['top', 'bottom'] as const)(
  'preserves the Android %s offset',
  async position => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await mount(
      <Toast
        offset={24}
        onHide={jest.fn()}
        position={position}
        toast={{ message: '位置提示' }}
      />,
    );
    const layer = screen.getByTestId('toast').parent;
    expect(StyleSheet.flatten(layer?.props.style)[position]).toBe(24);
  },
);

it('keeps an action toast reachable as the message and then the button', async () => {
  const announce = jest.mocked(AccessibilityInfo.announceForAccessibility);
  await mount(
    <Toast
      onHide={jest.fn()}
      toast={{
        message: '已删除',
        action: { label: '撤销', onPress: jest.fn() },
      }}
    />,
  );
  const toast = screen.getByTestId('toast');
  expect(toast.props.accessible).toBeUndefined();
  expect(toast.props.accessibilityLabel).toBeUndefined();
  expect(toast).toHaveProp('accessibilityLiveRegion', 'polite');
  expect(screen.getByText('已删除')).toBeOnTheScreen();
  const action = screen.getByRole('button', { name: '撤销' });
  expect(toast).toContainElement(action);
  expect(announce).toHaveBeenCalledWith('已删除');
});

it('gives the Android action a 48 dp target', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await mount(
    <Toast
      onHide={jest.fn()}
      toast={{
        message: '已删除',
        action: { label: '撤销', onPress: jest.fn() },
      }}
    />,
  );
  expect(
    StyleSheet.flatten(screen.getByTestId('toast-action').props.style),
  ).toMatchObject({ minHeight: 48 });
});
