import { render, screen } from '@testing-library/react-native';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { Banner } from './Banner';

describe('Banner accessibility', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('keeps action banners navigable as copy followed by the button', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    await render(
      <Banner
        action={{ label: '打开 Tailscale', onPress: jest.fn() }}
        message="打开 Tailscale 后再试"
        title="Tailscale 没有连接"
        tone="warn"
        testID="banner"
      />,
    );

    const button = screen.getByTestId('banner-action');
    let ancestor = button.parent;
    while (ancestor != null) {
      expect(ancestor.props.accessible).not.toBe(true);
      ancestor = ancestor.parent;
    }

    const copy = screen.getByTestId('banner-copy');
    expect(copy).toHaveProp(
      'accessibilityLabel',
      'Tailscale 没有连接，打开 Tailscale 后再试',
    );
    expect(copy).toHaveProp('accessibilityLiveRegion', 'polite');
    expect(copy).toHaveProp('role', 'status');
    expect(screen.getByTestId('banner')).not.toHaveProp('accessibilityLabel');
  });

  it('announces an iOS banner on mount and when its label changes', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const view = await render(
      <Banner message="说明" title="标题" tone="info" testID="banner" />,
    );
    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce).toHaveBeenLastCalledWith('标题，说明');

    await view.rerender(
      <Banner message="新说明" title="标题" tone="info" testID="banner" />,
    );
    expect(announce).toHaveBeenCalledTimes(2);
    expect(announce).toHaveBeenLastCalledWith('标题，新说明');
  });

  it('can disable the iOS announcement path on mount and label changes', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const view = await render(
      <Banner
        announce={false}
        message="说明"
        title="标题"
        tone="info"
        testID="banner"
      />,
    );
    expect(announce).not.toHaveBeenCalled();

    await view.rerender(
      <Banner
        announce={false}
        message="新说明"
        title="标题"
        tone="info"
        testID="banner"
      />,
    );
    expect(announce).not.toHaveBeenCalled();
  });

  it('does not use the iOS announcement path on Android', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    await render(<Banner title="标题" tone="info" />);
    expect(announce).not.toHaveBeenCalled();
  });

  it('stacks an action banner at the largest font scale', async () => {
    const fontScale = jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    const view = await render(
      <Banner
        action={{ label: '打开', onPress: jest.fn() }}
        title="标题"
        tone="info"
        testID="banner"
      />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('banner').props.style),
    ).toMatchObject({ flexDirection: 'column', alignItems: 'stretch' });

    fontScale.mockReturnValue(2);
    await view.rerender(
      <Banner
        action={{ label: '打开', onPress: jest.fn() }}
        title="标题"
        tone="info"
        testID="banner"
      />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('banner').props.style),
    ).toMatchObject({ flexDirection: 'row' });
  });
  it('keeps a dismissible banner navigable as copy followed by the close button', async () => {
    await render(
      <Banner
        message="打开 Tailscale 后再试"
        onDismiss={jest.fn()}
        title="Tailscale 没有连接"
        tone="warn"
        testID="banner"
      />,
    );

    const dismiss = screen.getByTestId('banner-dismiss');
    let ancestor = dismiss.parent;
    while (ancestor != null) {
      expect(ancestor.props.accessible).not.toBe(true);
      ancestor = ancestor.parent;
    }
    const copy = screen.getByTestId('banner-copy');
    expect(copy).toHaveProp(
      'accessibilityLabel',
      'Tailscale 没有连接，打开 Tailscale 后再试',
    );
    expect(copy).toHaveProp('role', 'status');
    expect(dismiss).toHaveProp('accessibilityLabel', '关闭提示');
  });
});
