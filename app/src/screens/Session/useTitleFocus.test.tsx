import { act, cleanup, render } from '@testing-library/react-native';
import { AccessibilityInfo, Text, View } from 'react-native';
import { useTitleFocus } from './useTitleFocus';

function Probe({ active = true }: { active?: boolean }) {
  const ref = useTitleFocus(active);
  return (
    <View ref={ref as never} accessible accessibilityRole="header">
      <Text>标题</Text>
    </View>
  );
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  jest.clearAllTimers();
  jest.useRealTimers();
  jest.restoreAllMocks();
});

it('focuses the mounted title on the next turn', async () => {
  const send = jest.spyOn(AccessibilityInfo, 'sendAccessibilityEvent');
  const view = await render(<Probe />);

  expect(send).not.toHaveBeenCalled();
  await act(async () => {
    jest.advanceTimersByTime(1);
    await Promise.resolve();
  });

  expect(send).toHaveBeenCalledWith(expect.anything(), 'focus');
  await view.unmount();
});

it('cleans up a pending focus when unmounted', async () => {
  const send = jest.spyOn(AccessibilityInfo, 'sendAccessibilityEvent');
  const view = await render(<Probe />);
  send.mockClear();
  await act(async () => {
    await view.unmount();
  });

  act(() => {
    jest.advanceTimersByTime(0);
  });

  expect(send).not.toHaveBeenCalled();
});

it('focuses when active changes from false to true', async () => {
  await cleanup();
  jest.clearAllTimers();
  const active = await render(<Probe active />);
  await act(async () => {
    jest.runOnlyPendingTimers();
  });

  expect(active).toBeDefined();
  await active.unmount();
});
