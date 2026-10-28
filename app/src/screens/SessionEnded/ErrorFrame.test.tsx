import { fireEvent, render, screen } from '@testing-library/react-native';
import { Text } from 'react-native';
import { mockWindowDimensions } from '../../test/dimensions';
import ErrorFrame from './ErrorFrame';

function frame(
  overrides: Partial<React.ComponentProps<typeof ErrorFrame>> = {},
) {
  return (
    <ErrorFrame
      code={101}
      desc="请允许屏幕录制和辅助功能。"
      kicker="缺少权限"
      onCopyCode={jest.fn()}
      title="这台电脑还没有获得系统权限"
      {...overrides}
    />
  );
}

describe('ErrorFrame', () => {
  it('exposes the title as a header', async () => {
    await render(frame());
    expect(
      screen.getByRole('header', { name: '这台电脑还没有获得系统权限' }),
    ).toBeOnTheScreen();
  });

  it('copies the RS-E code from its labelled button', async () => {
    const onCopyCode = jest.fn();
    await render(frame({ onCopyCode }));
    await fireEvent.press(screen.getByLabelText('复制错误码 RS-E101'));
    expect(onCopyCode).toHaveBeenCalledTimes(1);
  });

  it('hides the code button when there is no code', async () => {
    await render(frame({ code: null }));
    expect(screen.queryByText('RS-E101')).toBeNull();
    expect(screen.queryByLabelText('复制错误码 RS-E101')).toBeNull();
  });

  it('hides the illustration from assistive technology', async () => {
    await render(frame({ illustration: <Text testID="drawing">示意图</Text> }));
    const hidden = screen.getByTestId('session-ended-illustration', {
      includeHiddenElements: true,
    });
    expect(hidden.props.accessibilityElementsHidden).toBe(true);
    expect(hidden.props.importantForAccessibility).toBe('no-hide-descendants');
  });

  it('stacks the illustration with a slim gutter on a phone (V27)', async () => {
    const restore = mockWindowDimensions({ width: 375, height: 667 });
    try {
      await render(frame({ illustration: <Text>示意图</Text> }));
      expect(screen.getByTestId('session-ended-scroll')).toHaveStyle({
        left: 16,
        right: 16,
      });
      expect(
        screen.getByTestId('session-ended-illustration', {
          includeHiddenElements: true,
        }),
      ).toHaveStyle({ width: '100%' });
    } finally {
      restore();
    }
  });

  it('keeps the illustration beside the text when there is room', async () => {
    const restore = mockWindowDimensions({ width: 1024, height: 768 });
    try {
      await render(frame({ illustration: <Text>示意图</Text> }));
      expect(screen.getByTestId('session-ended-scroll')).toHaveStyle({
        left: 59,
        right: 59,
      });
      expect(
        screen.getByTestId('session-ended-illustration', {
          includeHiddenElements: true,
        }),
      ).toHaveStyle({ width: 250 });
    } finally {
      restore();
    }
  });

  it('runs primary, secondary and back actions', async () => {
    const primary = jest.fn();
    const secondary = jest.fn();
    const back = jest.fn();
    await render(
      frame({
        primary: { label: '已打开，重试', onPress: primary },
        secondary: { label: '改用屏幕共享', onPress: secondary },
        back: { label: '返回', onPress: back },
      }),
    );
    await fireEvent.press(screen.getByText('已打开，重试'));
    await fireEvent.press(screen.getByText('改用屏幕共享'));
    await fireEvent.press(screen.getByText('返回'));
    expect(primary).toHaveBeenCalledTimes(1);
    expect(secondary).toHaveBeenCalledTimes(1);
    expect(back).toHaveBeenCalledTimes(1);
  });
});
