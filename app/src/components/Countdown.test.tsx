import { act, render, screen } from '@testing-library/react-native';
import { StyleSheet, Text } from 'react-native';
import { Countdown, useCountdown } from './Countdown';

type HarnessProps = {
  total: number;
  running: boolean;
  onDone?: () => void;
  resetKey?: unknown;
};

function Harness({ total, running, onDone, resetKey }: HarnessProps) {
  const remaining = useCountdown(total, { running, onDone, key: resetKey });
  return <Text>{remaining}</Text>;
}

describe('useCountdown', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('ticks down, stops at zero and calls onDone once', async () => {
    const onDone = jest.fn();
    await render(<Harness onDone={onDone} running total={30} />);
    expect(screen.getByText('30')).toBeOnTheScreen();

    await act(async () => {
      jest.advanceTimersByTime(2000);
    });
    expect(screen.getByText('28')).toBeOnTheScreen();

    await act(async () => {
      jest.advanceTimersByTime(28000);
    });
    expect(screen.getByText('0')).toBeOnTheScreen();
    expect(onDone).toHaveBeenCalledTimes(1);

    await act(async () => {
      jest.advanceTimersByTime(5000);
    });
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it('pauses while running is false', async () => {
    const { rerender } = await render(<Harness running total={30} />);
    await act(async () => {
      jest.advanceTimersByTime(3000);
    });
    expect(screen.getByText('27')).toBeOnTheScreen();

    await rerender(<Harness running={false} total={30} />);
    await act(async () => {
      jest.advanceTimersByTime(10000);
    });
    expect(screen.getByText('27')).toBeOnTheScreen();
  });

  it('resets when the key changes', async () => {
    const { rerender } = await render(
      <Harness resetKey="first" running total={30} />,
    );
    await act(async () => {
      jest.advanceTimersByTime(5000);
    });
    expect(screen.getByText('25')).toBeOnTheScreen();

    await rerender(<Harness resetKey="second" running total={30} />);
    expect(screen.getByText('30')).toBeOnTheScreen();
  });

  it('cleans up its interval on unmount', async () => {
    const setIntervalSpy = jest.spyOn(globalThis, 'setInterval');
    const clearIntervalSpy = jest.spyOn(globalThis, 'clearInterval');
    try {
      const { unmount } = await render(<Harness running total={30} />);
      const intervalId = setIntervalSpy.mock.results[0]?.value;

      await unmount();
      expect(clearIntervalSpy).toHaveBeenCalledWith(intervalId);
    } finally {
      setIntervalSpy.mockRestore();
      clearIntervalSpy.mockRestore();
    }
  });
});

describe('Countdown', () => {
  it('renders the progress ring, mono digits and seconds label', async () => {
    await render(<Countdown remaining={28} total={30} />);

    const timer = screen.getByLabelText('倒计时 28 秒');
    expect(timer).toHaveProp('accessibilityLabel', '倒计时 28 秒');
    expect(timer).toHaveProp('accessibilityRole', 'timer');
    expect(screen.getByText('28')).toHaveStyle({
      fontFamily: 'GeistMono-SemiBold',
      fontSize: 13,
      lineHeight: 18,
    });
    expect(screen.getByText('秒')).toHaveStyle({
      fontSize: 11,
      lineHeight: 14,
    });

    const circles = timer.queryAll(node => node.type === 'RNSVGCircle');
    expect(circles).toHaveLength(2);
    expect(circles[0].props.stroke).toEqual({
      type: 0,
      payload: expect.anything(),
    });
    expect(StyleSheet.flatten(timer.props.style)).toMatchObject({
      width: 36,
      height: 36,
    });
    expect(circles[1].props.strokeDasharray).toBeDefined();
  });

  it('keeps the progress circle at zero remaining', async () => {
    await render(<Countdown remaining={0} total={30} />);

    const timer = screen.getByLabelText('倒计时 0 秒');
    expect(timer.queryAll(node => node.type === 'RNSVGCircle')).toHaveLength(2);
  });
});
