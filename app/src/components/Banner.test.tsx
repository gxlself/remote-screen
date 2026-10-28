import { fireEvent, render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { Banner, type BannerTone } from './Banner';

const tones: Array<[BannerTone, string, string]> = [
  ['warn', 'rgba(244,178,78,0.1)', 'rgba(244,178,78,0.3)'],
  ['good', 'rgba(93,211,154,0.08)', 'rgba(93,211,154,0.28)'],
  ['info', 'rgba(125, 168, 255, 0.08)', 'rgba(125,168,255,0.3)'],
  ['bad', 'rgba(255,136,115,0.1)', 'rgba(255,136,115,0.3)'],
];

describe('Banner', () => {
  it.each(tones)(
    'renders the %s tone with its canvas colours',
    async (tone, backgroundColor, borderColor) => {
      await render(
        <Banner title="VPN 状态" message="说明" tone={tone} testID="banner" />,
      );

      expect(
        StyleSheet.flatten(screen.getByTestId('banner').props.style),
      ).toMatchObject({
        minHeight: 64,
        borderRadius: 16,
        borderWidth: 1,
        paddingTop: 12,
        paddingRight: 12,
        paddingBottom: 12,
        paddingLeft: 14,
        gap: 12,
        backgroundColor,
        borderColor,
      });
    },
  );

  it('renders the 14 px semibold title and 12 px secondary copy', async () => {
    await render(
      <Banner
        title="Tailscale 没有连接"
        message="打开 Tailscale 后再试"
        tone="warn"
      />,
    );

    expect(screen.getByText('Tailscale 没有连接')).toHaveStyle({
      fontSize: 14,
      lineHeight: 20,
      fontFamily: 'Geist-SemiBold',
    });
    expect(screen.getByText('打开 Tailscale 后再试')).toHaveStyle({
      fontSize: 12,
      lineHeight: 17,
      color: '#A9B4BB',
    });
  });

  it('renders a 32 px icon circle with an 18 px icon', async () => {
    await render(
      <Banner title="VPN 状态" icon="warning" tone="warn" testID="banner" />,
    );

    expect(
      StyleSheet.flatten(screen.getByTestId('banner-icon-wrap').props.style),
    ).toMatchObject({ width: 32, height: 32, borderRadius: 16 });
    const icon = screen.getByTestId('banner-icon', {
      includeHiddenElements: true,
    });
    expect(icon.props.bbWidth).toBe(18);
  });

  it('calls the action and forwards its disabled state', async () => {
    const onPress = jest.fn();
    await render(
      <Banner
        action={{ label: '打开 Tailscale', onPress }}
        title="VPN 状态"
        tone="info"
        testID="banner"
      />,
    );

    await fireEvent.press(screen.getByTestId('banner-action'));
    expect(onPress).toHaveBeenCalledTimes(1);

    await screen.rerender(
      <Banner
        action={{ label: '打开 Tailscale', onPress, disabled: true }}
        title="VPN 状态"
        tone="info"
        testID="banner"
      />,
    );
    await fireEvent.press(screen.getByTestId('banner-action'));
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('announces the title and message together in a polite live region', async () => {
    await render(
      <Banner
        message="系统 Tailscale 被断开了，iOS 同时只能开一个系统 VPN"
        title="公司 VPN 已连接"
        tone="warn"
        testID="banner"
      />,
    );

    const banner = screen.getByTestId('banner');
    expect(banner).toHaveProp(
      'accessibilityLabel',
      '公司 VPN 已连接，系统 Tailscale 被断开了，iOS 同时只能开一个系统 VPN',
    );
    expect(banner).toHaveProp('accessibilityLiveRegion', 'polite');
    expect(banner).toHaveProp('role', 'status');
  });
  it('shows a busy action button while loading', async () => {
    const onPress = jest.fn();
    await render(
      <Banner
        action={{ label: '打开 Tailscale', onPress, loading: true }}
        title="VPN 状态"
        tone="info"
        testID="banner"
      />,
    );
    const action = screen.getByTestId('banner-action');
    expect(action).toBeBusy();
    await fireEvent.press(action);
    expect(onPress).not.toHaveBeenCalled();
  });

  it('adds a labelled close button only when onDismiss is given', async () => {
    const onDismiss = jest.fn();
    const view = await render(
      <Banner title="VPN 状态" tone="info" testID="banner" />,
    );
    expect(screen.queryByTestId('banner-dismiss')).toBeNull();

    await view.rerender(
      <Banner
        onDismiss={onDismiss}
        title="VPN 状态"
        tone="info"
        testID="banner"
      />,
    );
    const dismiss = screen.getByRole('button', { name: '关闭提示' });
    await fireEvent.press(dismiss);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
