import { fireEvent, render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { HudPill } from './HudPill';

function pillStyle() {
  return StyleSheet.flatten(screen.getByTestId('pill').props.style);
}

function latencyStyle() {
  return StyleSheet.flatten(screen.getByText(/ ms$/).props.style);
}

describe('HudPill', () => {
  it('renders an expanded direct path pill with coloured latency', async () => {
    await render(
      <HudPill
        accessibilityLabel="连接路径，直连 6 毫秒，60 帧"
        fps={60}
        latencyMs={6}
        mode="expanded"
        path="tailnetDirect"
        testID="pill"
      />,
    );

    expect(pillStyle()).toMatchObject({
      height: 32,
      borderRadius: 16,
      paddingHorizontal: 12,
      gap: 7,
      backgroundColor: 'rgba(10, 13, 16, 0.8)',
      borderColor: 'rgba(255, 255, 255, 0.08)',
      borderWidth: 1,
    });
    expect(screen.getByText('直连')).toBeOnTheScreen();
    expect(screen.getByText('6 ms')).toBeOnTheScreen();
    expect(screen.getByText('60 fps')).toBeOnTheScreen();
    expect(latencyStyle().color).toBe('#5DD39A');
  });

  it('uses the amber latency colour for relay paths', async () => {
    await render(
      <HudPill
        accessibilityLabel="中继 48 毫秒"
        latencyMs={48}
        mode="expanded"
        path="tailnetDerp"
        testID="pill"
      />,
    );

    expect(latencyStyle().color).toBe('#F4B24E');
  });

  it('renders a custom intranet label, TCP tag and blue latency', async () => {
    await render(
      <HudPill
        accessibilityLabel="内网 VPN 44 毫秒 TCP"
        latencyMs={44}
        mode="expanded"
        path="intranet"
        pathText="内网 · VPN"
        tcp
        testID="pill"
      />,
    );

    expect(screen.getByText('内网 · VPN')).toBeOnTheScreen();
    expect(screen.getByText('TCP')).toBeOnTheScreen();
    expect(latencyStyle().color).toBe('#A9C3FF');
  });

  it('collapses to a 32 px mark-only button', async () => {
    await render(
      <HudPill
        accessibilityLabel="展开会话状态"
        latencyMs={6}
        mode="collapsed"
        path="tailnetDirect"
        pathText="直连"
        testID="pill"
      />,
    );

    expect(pillStyle()).toMatchObject({
      width: 32,
      height: 32,
      borderRadius: 16,
    });
    expect(screen.queryByText('直连')).toBeNull();
    expect(screen.queryByText('6 ms')).toBeNull();
  });

  it('renders the alert copy on the amber background', async () => {
    await render(
      <HudPill
        accessibilityLabel="画质已降档"
        alertText="已降到 1080p30"
        mode="alert"
        path="tailnetDerp"
        testID="pill"
      />,
    );

    expect(screen.getByText('已降到 1080p30')).toBeOnTheScreen();
    expect(pillStyle().backgroundColor).toBe('rgba(244,178,78,0.92)');
  });

  it('acts as one button with the caller supplied label', async () => {
    const onPress = jest.fn();
    await render(
      <HudPill
        accessibilityLabel="打开画质面板"
        mode="expanded"
        onPress={onPress}
        path="lan"
        testID="pill"
      />,
    );

    const pill = screen.getByTestId('pill');
    expect(pill).toHaveProp('accessibilityLabel', '打开画质面板');
    expect(pill).toHaveProp('accessibilityRole', 'button');
    await fireEvent.press(pill);
    expect(onPress).toHaveBeenCalledTimes(1);
  });
});
