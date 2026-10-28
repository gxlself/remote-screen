import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
} from '@testing-library/react-native';
import { AccessibilityInfo, StyleSheet, Text } from 'react-native';
import { Toast, useToast } from './Toast';

const undo = jest.fn();

function UseToastHarness({ durationMs }: { durationMs?: number }) {
  const { toast, show, hide } = useToast(durationMs);
  return (
    <>
      <Toast onHide={hide} toast={toast} />
      <Text onPress={() => show('已复制')}>show-copy</Text>
      <Text onPress={() => show('再试', { tone: 'bad' })}>show-retry</Text>
      <Text
        onPress={() =>
          show('已删除', { action: { label: '撤销', onPress: undo } })
        }
      >
        show-undo
      </Text>
      <Text onPress={hide}>hide</Text>
    </>
  );
}

describe('useToast and Toast', () => {
  beforeEach(() => {
    jest.useFakeTimers();
    undo.mockClear();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('shows, announces once and auto-hides after the 3000 ms default', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    announce.mockClear();
    await render(<UseToastHarness />);

    await fireEvent.press(screen.getByText('show-copy'));
    expect(screen.getByText('已复制')).toBeOnTheScreen();
    expect(announce).toHaveBeenCalledWith('已复制');
    expect(announce).toHaveBeenCalledTimes(1);

    await act(async () => {
      jest.advanceTimersByTime(2999);
    });
    expect(screen.getByText('已复制')).toBeOnTheScreen();

    await act(async () => {
      jest.advanceTimersByTime(1);
    });
    expect(screen.queryByText('已复制')).toBeNull();
  });

  it('restarts the timer when shown again', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    announce.mockClear();
    await render(<UseToastHarness />);

    await fireEvent.press(screen.getByText('show-copy'));
    await act(async () => {
      jest.advanceTimersByTime(2000);
    });
    await fireEvent.press(screen.getByText('show-retry'));
    expect(screen.getByText('再试')).toBeOnTheScreen();
    expect(announce).toHaveBeenLastCalledWith('再试');

    await act(async () => {
      jest.advanceTimersByTime(2999);
    });
    expect(screen.getByText('再试')).toBeOnTheScreen();
    await act(async () => {
      jest.advanceTimersByTime(1);
    });
    expect(screen.queryByText('再试')).toBeNull();
  });

  it('hides immediately and cleans up its timer on unmount', async () => {
    const view = await render(<UseToastHarness />);
    await fireEvent.press(screen.getByText('show-copy'));
    await fireEvent.press(screen.getByText('hide'));
    expect(screen.queryByText('已复制')).toBeNull();

    await fireEvent.press(screen.getByText('show-copy'));
    await view.unmount();
    await act(async () => {
      jest.advanceTimersByTime(5000);
    });
  });

  it('matches the HUD look, tone icon and bottom default position', async () => {
    const onHide = jest.fn();
    await render(
      <Toast onHide={onHide} toast={{ message: '已复制', tone: 'good' }} />,
    );

    const toast = screen.getByTestId('toast');
    expect(toast).toHaveProp('accessibilityLiveRegion', 'polite');
    expect(StyleSheet.flatten(toast.props.style)).toMatchObject({
      minHeight: 40,
      maxWidth: 480,
      borderRadius: 20,
      paddingHorizontal: 14,
      borderWidth: 1,
      borderColor: 'rgba(255, 255, 255, 0.08)',
      backgroundColor: 'rgba(10, 13, 16, 0.8)',
    });
    expect(
      StyleSheet.flatten(screen.getByText('已复制').props.style),
    ).toMatchObject({
      fontSize: 13,
      lineHeight: 19,
      fontFamily: 'Geist-Medium',
    });
  });

  it('supports the top position and the useToast return contract', async () => {
    const hook = await renderHook(() => useToast());
    expect(hook.result.current.toast).toBeNull();
    expect(typeof hook.result.current.show).toBe('function');
    expect(typeof hook.result.current.hide).toBe('function');

    await render(
      <Toast
        onHide={jest.fn()}
        offset={24}
        position="top"
        toast={{ message: '顶部' }}
      />,
    );
    expect(screen.getByText('顶部')).toBeOnTheScreen();
  });

  it('announces a repeated message again and clears the timer on hide', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    announce.mockClear();
    await render(<UseToastHarness />);

    await fireEvent.press(screen.getByText('show-copy'));
    await fireEvent.press(screen.getByText('show-copy'));
    expect(announce).toHaveBeenCalledTimes(2);

    await fireEvent.press(screen.getByText('hide'));
    await act(async () => {
      jest.advanceTimersByTime(3000);
    });
    expect(announce).toHaveBeenCalledTimes(2);
  });

  it('runs the action once and hides the toast', async () => {
    await render(<UseToastHarness />);
    await fireEvent.press(screen.getByText('show-undo'));

    const action = screen.getByRole('button', { name: '撤销' });
    await fireEvent.press(action);
    expect(undo).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('已删除')).toBeNull();
  });

  it('keeps an action toast up for 6 s instead of 3 s', async () => {
    await render(<UseToastHarness />);
    await fireEvent.press(screen.getByText('show-undo'));

    await act(async () => {
      jest.advanceTimersByTime(5999);
    });
    expect(screen.getByText('已删除')).toBeOnTheScreen();
    await act(async () => {
      jest.advanceTimersByTime(1);
    });
    expect(screen.queryByText('已删除')).toBeNull();
    expect(undo).not.toHaveBeenCalled();
  });

  it('lets touches through a plain toast but not an action toast', async () => {
    const view = await render(
      <Toast onHide={jest.fn()} toast={{ message: '已复制' }} />,
    );
    expect(screen.getByTestId('toast').parent).toHaveProp(
      'pointerEvents',
      'none',
    );
    expect(screen.queryByRole('button')).toBeNull();

    await view.rerender(
      <Toast
        onHide={jest.fn()}
        toast={{ message: '已删除', action: { label: '撤销', onPress: undo } }}
      />,
    );
    expect(screen.getByTestId('toast').parent).toHaveProp(
      'pointerEvents',
      'box-none',
    );
  });

  it('hides before running the action, so the action can raise a new toast', async () => {
    const calls: string[] = [];
    await render(
      <Toast
        onHide={() => calls.push('hide')}
        testID="undo-toast"
        toast={{
          message: '已删除',
          action: { label: '撤销', onPress: () => calls.push('undo') },
        }}
      />,
    );
    await fireEvent.press(screen.getByTestId('undo-toast-action'));
    expect(calls).toEqual(['hide', 'undo']);
  });
});
