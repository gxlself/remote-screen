import { useEffect, type ComponentRef, type RefObject } from 'react';
import type { ScrollView } from 'react-native';

/** What a `ref` on a `ScrollView` holds. */
export type ScrollViewHandle = ComponentRef<typeof ScrollView>;

type TabNavigation = {
  addListener(type: 'tabPress', callback: () => void): () => void;
  isFocused(): boolean;
};

/**
 * Tapping the tab that is already open scrolls its list back to the top,
 * like `useScrollToTop`, which needs a navigator above it.
 */
export function useTabScrollToTop(
  navigation: TabNavigation | undefined,
  ref: RefObject<ScrollViewHandle | null>,
): void {
  useEffect(() => {
    if (navigation === undefined) {
      return;
    }
    return navigation.addListener('tabPress', () => {
      if (navigation.isFocused()) {
        ref.current?.scrollTo({ y: 0, animated: true });
      }
    });
  }, [navigation, ref]);
}
