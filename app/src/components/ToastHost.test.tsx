import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { useState } from 'react';
import { AccessibilityInfo, Platform, Text } from 'react-native';
import { ToastHost, hideToast, showToast } from './ToastHost';

function Editor({ onDone }: { onDone: () => void }) {
  return (
    <Text
      onPress={() => {
        showToast({ message: '已保存', tone: 'good' });
        onDone();
      }}
    >
      save
    </Text>
  );
}

function App() {
  const [editing, setEditing] = useState(true);
  return (
    <>
      {editing ? <Editor onDone={() => setEditing(false)} /> : null}
      <ToastHost />
    </>
  );
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  jest.mocked(AccessibilityInfo.announceForAccessibility).mockClear();
});

afterEach(async () => {
  // The toast lives in module state; do not let one test's toast leak.
  await act(async () => hideToast());
  jest.useRealTimers();
});

it('renders nothing until a toast is shown', async () => {
  await render(<ToastHost />);
  expect(screen.queryByTestId('global-toast')).toBeNull();
});

it('keeps a toast up after the screen that raised it has gone', async () => {
  await render(<App />);
  await fireEvent.press(screen.getByText('save'));

  expect(screen.queryByText('save')).toBeNull();
  expect(screen.getByTestId('global-toast')).toHaveProp(
    'accessibilityLabel',
    '已保存',
  );
  expect(AccessibilityInfo.announceForAccessibility).toHaveBeenCalledWith(
    '已保存',
  );

  await act(async () => {
    jest.advanceTimersByTime(2999);
  });
  expect(screen.getByText('已保存')).toBeOnTheScreen();
  await act(async () => {
    jest.advanceTimersByTime(1);
  });
  expect(screen.queryByTestId('global-toast')).toBeNull();
});

it('runs an action once, hides, and stays 6 s when untouched', async () => {
  const undo = jest.fn();
  await render(<ToastHost />);

  await act(() =>
    showToast({ message: '已删除', action: { label: '撤销', onPress: undo } }),
  );
  await act(async () => {
    jest.advanceTimersByTime(5999);
  });
  await fireEvent.press(screen.getByRole('button', { name: '撤销' }));
  expect(undo).toHaveBeenCalledTimes(1);
  expect(screen.queryByTestId('global-toast')).toBeNull();

  await act(() =>
    showToast({ message: '已删除', action: { label: '撤销', onPress: undo } }),
  );
  await act(async () => {
    jest.advanceTimersByTime(6000);
  });
  expect(screen.queryByTestId('global-toast')).toBeNull();
  expect(undo).toHaveBeenCalledTimes(1);
});

it('replaces the current toast, restarts its timer and announces again', async () => {
  await render(<ToastHost />);
  await act(() => showToast({ message: '已复制' }));
  await act(async () => {
    jest.advanceTimersByTime(2000);
  });
  await act(() => showToast({ message: '已复制' }));
  expect(AccessibilityInfo.announceForAccessibility).toHaveBeenCalledTimes(2);

  await act(async () => {
    jest.advanceTimersByTime(2999);
  });
  expect(screen.getByText('已复制')).toBeOnTheScreen();
  await act(async () => {
    jest.advanceTimersByTime(1);
  });
  expect(screen.queryByText('已复制')).toBeNull();
});

it('honours a custom duration and hides on demand', async () => {
  await render(<ToastHost />);
  await act(() => showToast({ message: '正在连接', durationMs: 10000 }));
  await act(async () => {
    jest.advanceTimersByTime(9999);
  });
  expect(screen.getByText('正在连接')).toBeOnTheScreen();

  await act(() => hideToast());
  expect(screen.queryByText('正在连接')).toBeNull();
  await act(async () => {
    jest.advanceTimersByTime(10000);
  });
  expect(screen.queryByTestId('global-toast')).toBeNull();
});
