import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import { AccessibilityInfo, PixelRatio, StyleSheet } from 'react-native';
import { renderWithProviders } from '../test/render';
import AppText from './AppText';
import { SessionPanel, type SessionPanelKind } from './SessionPanel';

let view: Awaited<ReturnType<typeof renderWithProviders>> | undefined;

async function mount(ui: ReactElement) {
  view = await renderWithProviders(ui);
  return view;
}

function panel(kind: SessionPanelKind, onClose = jest.fn()) {
  return (
    <SessionPanel kind={kind} onClose={onClose} testID="panel" title="画质选项">
      <AppText>面板内容</AppText>
    </SessionPanel>
  );
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.mocked(AccessibilityInfo.sendAccessibilityEvent).mockClear();
});

afterEach(async () => {
  await cleanup();
  view?.queryClient.clear();
  view = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it.each(['side', 'popover'] as const)(
  'focuses the %s title View on mount',
  async kind => {
    const focus = jest.mocked(AccessibilityInfo.sendAccessibilityEvent);
    await mount(panel(kind));
    const title = screen.getByRole('header', { name: '画质选项' });
    expect(title.type).toBe('View');
    expect(title).toHaveProp('accessible', true);
    expect(focus).not.toHaveBeenCalled();
    await act(() => jest.advanceTimersByTimeAsync(0));
    expect(focus).toHaveBeenCalledWith(
      title.unstable_fiber?.return?.stateNode,
      'focus',
    );
    expect(focus).toHaveBeenCalledTimes(1);
  },
);

it.each(['side', 'popover'] as const)(
  'closes the %s panel from accessibility escape',
  async kind => {
    const onClose = jest.fn();
    await mount(panel(kind, onClose));
    await fireEvent(screen.getByTestId('panel'), 'accessibilityEscape');
    expect(onClose).toHaveBeenCalledTimes(1);
  },
);

it('cancels pending title focus on unmount', async () => {
  const mounted = await mount(panel('side'));
  await mounted.unmount();
  await act(() => jest.advanceTimersByTimeAsync(0));
  expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
});

it('lets the visible title header grow at font scale 3', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  await mount(panel('side'));
  const title = screen.getByRole('header', { name: '画质选项' });
  const headerStyle = StyleSheet.flatten(title.parent?.parent?.props.style);
  expect(headerStyle).toMatchObject({ minHeight: 44 });
  expect(headerStyle.height).toBeUndefined();
  expect(screen.getByText('画质选项').props.numberOfLines).toBeUndefined();
  expect(screen.getByText('画质选项')).toHaveProp('allowFontScaling', true);
});
