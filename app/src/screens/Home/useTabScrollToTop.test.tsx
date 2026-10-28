import { renderHook } from '@testing-library/react-native';
import { useTabScrollToTop, type ScrollViewHandle } from './useTabScrollToTop';

function tabNavigation(focused: boolean) {
  let listener: (() => void) | undefined;
  const unsubscribe = jest.fn();
  return {
    navigation: {
      addListener: jest.fn((type: 'tabPress', callback: () => void) => {
        expect(type).toBe('tabPress');
        listener = callback;
        return unsubscribe;
      }),
      isFocused: jest.fn(() => focused),
    },
    press: () => listener?.(),
    unsubscribe,
  };
}

function scrollRef() {
  const scrollTo = jest.fn();
  const ref = {
    current: { scrollTo } as unknown as ScrollViewHandle,
  };
  return { ref, scrollTo };
}

it('scrolls back to the top when the open tab is pressed again', async () => {
  const tab = tabNavigation(true);
  const { ref, scrollTo } = scrollRef();
  const hook = await renderHook(() => useTabScrollToTop(tab.navigation, ref));

  tab.press();
  expect(scrollTo).toHaveBeenCalledWith({ y: 0, animated: true });

  await hook.unmount();
  expect(tab.unsubscribe).toHaveBeenCalledTimes(1);
});

it('leaves the list alone when the press switches to this tab', async () => {
  const tab = tabNavigation(false);
  const { ref, scrollTo } = scrollRef();
  await renderHook(() => useTabScrollToTop(tab.navigation, ref));

  tab.press();
  expect(scrollTo).not.toHaveBeenCalled();
});

it('does nothing without a navigator', async () => {
  const { ref, scrollTo } = scrollRef();
  await renderHook(() => useTabScrollToTop(undefined, ref));

  expect(scrollTo).not.toHaveBeenCalled();
});
