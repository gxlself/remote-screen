import { useEffect, useRef } from 'react';
import type React from 'react';
import { AccessibilityInfo, View } from 'react-native';
import type { HostInstance } from 'react-native';

export function useTitleFocus(active = true): React.RefObject<View | null> {
  const ref = useRef<View | null>(null);

  useEffect(() => {
    if (!active) {
      return undefined;
    }
    const timer = setTimeout(() => {
      if (ref.current !== null) {
        AccessibilityInfo.sendAccessibilityEvent(
          ref.current as unknown as HostInstance,
          'focus',
        );
      }
    }, 0);
    return () => clearTimeout(timer);
  }, [active]);

  return ref;
}
