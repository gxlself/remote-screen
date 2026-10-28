import { render, screen } from '@testing-library/react-native';
import { AccessibilityInfo, PixelRatio, StyleSheet } from 'react-native';
import { Countdown } from './Countdown';

describe('Countdown accessibility', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('moves the value beside the ring at a large font scale', async () => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    const view = await render(
      <Countdown remaining={28} size={36} total={30} />,
    );

    const timer = screen.getByLabelText('倒计时 28 秒');
    const timerStyle = StyleSheet.flatten(timer.props.style);
    expect(timerStyle.width).toBeUndefined();
    expect(timerStyle.height).toBeUndefined();
    expect(screen.getByText('28 秒')).toBeOnTheScreen();
    expect(
      timer.queryAll(node => {
        const style = StyleSheet.flatten(node.props.style);
        return (
          node.type === 'View' && style.width === 36 && style.height === 36
        );
      }),
    ).toHaveLength(1);

    await view.rerender(<Countdown remaining={28} size={60} total={30} />);
    const largeRing = screen.getByLabelText('倒计时 28 秒').queryAll(node => {
      const style = StyleSheet.flatten(node.props.style);
      return node.type === 'View' && style.width === 60 && style.height === 60;
    });
    expect(largeRing).toHaveLength(1);
  });

  it('never puts the changing value in a live region or announces ticks', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const fontScale = jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(2);
    const view = await render(
      <Countdown remaining={28} size={36} total={30} />,
    );
    expect(screen.getByLabelText('倒计时 28 秒')).not.toHaveProp(
      'accessibilityLiveRegion',
    );
    await view.rerender(<Countdown remaining={27} size={36} total={30} />);
    expect(screen.getByLabelText('倒计时 27 秒')).toBeOnTheScreen();
    expect(announce).not.toHaveBeenCalled();

    fontScale.mockReturnValue(3);
    await view.rerender(<Countdown remaining={27} size={36} total={30} />);
    expect(screen.getByLabelText('倒计时 27 秒')).not.toHaveProp(
      'accessibilityLiveRegion',
    );
  });
});
