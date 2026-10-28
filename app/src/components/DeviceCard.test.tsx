import { fireEvent, render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { DeviceCard, deviceCardA11yLabel } from './DeviceCard';

describe('deviceCardA11yLabel', () => {
  it('speaks the path, latency and display count', () => {
    expect(
      deviceCardA11yLabel({
        name: 'Studio Mac mini',
        path: 'tailnetDirect',
        latencyMs: 6,
        displays: 2,
      }),
    ).toBe('Studio Mac mini，直连 6 毫秒，2 块屏幕');
  });

  it('omits the display count for one screen', () => {
    expect(
      deviceCardA11yLabel({
        name: 'Studio Mac mini',
        path: 'tailnetDirect',
        latencyMs: 6,
        displays: 1,
      }),
    ).toBe('Studio Mac mini，直连 6 毫秒');
  });

  it('supports a spoken path override', () => {
    expect(
      deviceCardA11yLabel({
        name: '工位 Windows',
        path: 'tailnetDerp',
        a11yPath: '经上海中继',
        latencyMs: 48,
      }),
    ).toBe('工位 Windows，经上海中继，48 毫秒');
  });

  it('replaces the path with status text and appends the chip', () => {
    expect(
      deviceCardA11yLabel({
        name: '小明的 MacBook Pro',
        path: 'tailnetDirect',
        statusText: '轻点唤醒并连接',
      }),
    ).toBe('小明的 MacBook Pro，轻点唤醒并连接');
    expect(
      deviceCardA11yLabel({
        name: 'Studio Mac mini',
        path: 'tailnetDirect',
        latencyMs: 6,
        chip: { label: '控制中', tone: 'hostControlled' },
      }),
    ).toBe('Studio Mac mini，直连 6 毫秒，控制中');
  });
});

describe('DeviceCard', () => {
  it('is one accessible button and forwards press and long press', async () => {
    const onPress = jest.fn();
    const onLongPress = jest.fn();
    await render(
      <DeviceCard
        name="Studio Mac mini"
        look="live"
        path="tailnetDirect"
        latencyMs={6}
        displays={2}
        onPress={onPress}
        onLongPress={onLongPress}
        testID="card"
      />,
    );

    const card = screen.getByLabelText(
      'Studio Mac mini，直连 6 毫秒，2 块屏幕',
    );
    expect(card).toHaveProp('accessibilityRole', 'button');
    expect(card).toHaveProp(
      'accessibilityHint',
      '轻点两下开始远控，长按看详情',
    );
    await fireEvent.press(card);
    expect(onPress).toHaveBeenCalledTimes(1);
    await fireEvent(card, 'longPress');
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('renders the live and display badges', async () => {
    await render(
      <DeviceCard
        name="Studio Mac mini"
        look="live"
        live
        displays={2}
        onPress={jest.fn()}
      />,
    );
    expect(screen.getByText('实时')).toBeOnTheScreen();
    expect(screen.getByText('2 屏')).toBeOnTheScreen();
  });

  it('renders the sleeping thumbnail state', async () => {
    await render(
      <DeviceCard name="Studio Mac mini" look="sleeping" onPress={jest.fn()} />,
    );
    expect(screen.getByText('睡眠中')).toBeOnTheScreen();
  });

  it('uses a dashed offline frame and shows the offline state', async () => {
    await render(
      <DeviceCard
        name="Studio Mac mini"
        look="offline"
        onPress={jest.fn()}
        testID="card"
      />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('card').props.style),
    ).toMatchObject({
      backgroundColor: '#12171A',
      borderColor: '#2A333A',
      borderStyle: 'dashed',
    });
    expect(screen.getByText('离线')).toBeOnTheScreen();
  });

  it('replaces the path row with status text and its tone', async () => {
    await render(
      <DeviceCard
        name="Studio Mac mini"
        look="sleeping"
        path="tailnetDirect"
        latencyMs={6}
        statusText="轻点唤醒并连接"
        statusTone="accent"
        onPress={jest.fn()}
      />,
    );
    expect(screen.queryByText('直连')).toBeNull();
    expect(
      StyleSheet.flatten(screen.getByText('轻点唤醒并连接').props.style).color,
    ).toBe('#7DA8FF');
  });

  it('never truncates the device name', async () => {
    await render(
      <DeviceCard name="Studio Mac mini" look="live" onPress={jest.fn()} />,
    );
    expect(
      screen.getByText('Studio Mac mini').props.numberOfLines,
    ).toBeUndefined();
  });
});
