import { fireEvent, render, screen } from '@testing-library/react-native';
import { StyleSheet, Text } from 'react-native';
import { mockWindowDimensions } from '../test/dimensions';
import { SessionPanel } from './SessionPanel';

const children = <Text>面板内容</Text>;

function flatStyle(testID: string) {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
}

describe('SessionPanel', () => {
  it('renders a side panel title and closes from its button', async () => {
    const onClose = jest.fn();
    await render(
      <SessionPanel kind="side" onClose={onClose} title="画质" subtitle="自动">
        {children}
      </SessionPanel>,
    );

    expect(screen.getByRole('header')).toHaveTextContent('画质');
    expect(screen.getByText('自动')).toBeOnTheScreen();
    expect(screen.getByText('面板内容')).toBeOnTheScreen();
    await fireEvent.press(screen.getByLabelText('关闭'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('uses the side geometry, safe area and token colours', async () => {
    const restore = mockWindowDimensions({ width: 2000, height: 1000 });
    try {
      await render(
        <SessionPanel
          kind="side"
          onClose={jest.fn()}
          testID="quality"
          title="画质"
        >
          <Text>内容</Text>
        </SessionPanel>,
      );

      expect(flatStyle('quality-panel')).toMatchObject({
        width: 540,
        right: 59,
        top: 10,
        bottom: 10,
        borderRadius: 22,
        borderWidth: 1,
        borderColor: '#262E34',
        backgroundColor: 'rgba(21, 26, 30, 0.97)',
        paddingTop: 12,
        paddingHorizontal: 16,
        paddingBottom: 14,
        gap: 8,
      });
      expect(screen.getByTestId('quality')).toHaveProp(
        'accessibilityViewIsModal',
        true,
      );
    } finally {
      restore();
    }
  });

  it('shrinks below the 540 cap and uses a 12 px portrait right inset', async () => {
    const restore = mockWindowDimensions({ width: 500, height: 900 });
    try {
      await render(
        <SessionPanel
          kind="side"
          onClose={jest.fn()}
          testID="narrow"
          title="画质"
        >
          <Text>内容</Text>
        </SessionPanel>,
      );
      expect(flatStyle('narrow-panel')).toMatchObject({
        width: 500 - 71,
        right: 12,
      });
    } finally {
      restore();
    }
  });

  it('renders a popover with a visually hidden header and labelled backdrop', async () => {
    const onClose = jest.fn();
    await render(
      <SessionPanel
        backdropLabel="关闭剪贴板，回到画面"
        caretX={317}
        kind="popover"
        onClose={onClose}
        testID="clipboard"
        title="剪贴板"
      >
        {children}
      </SessionPanel>,
    );

    const header = screen.getByRole('header');
    expect(header).toHaveTextContent('剪贴板');
    expect(StyleSheet.flatten(header.props.style)).toMatchObject({
      width: 1,
      height: 1,
      opacity: 0,
    });

    const backdrop = screen.getByTestId('clipboard-backdrop');
    expect(backdrop).toHaveProp('accessibilityLabel', '关闭剪贴板，回到画面');
    await fireEvent.press(backdrop);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(
      StyleSheet.flatten(
        screen.getByTestId('clipboard-caret', {
          includeHiddenElements: true,
        }).props.style,
      ).left,
    ).toBe(317);
  });

  it('defaults the popover backdrop label to close', async () => {
    await render(
      <SessionPanel kind="popover" onClose={jest.fn()} title="更多">
        {children}
      </SessionPanel>,
    );

    const backdrop = screen.getByTestId('session-panel-backdrop');
    expect(backdrop).toHaveProp('accessibilityLabel', '关闭');
  });
});
