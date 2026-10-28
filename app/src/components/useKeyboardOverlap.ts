import { useEffect, useRef, useState } from 'react';
import { Keyboard, Platform, useWindowDimensions } from 'react-native';
import type { KeyboardEvent, KeyboardMetrics } from 'react-native';

function covered(frame: KeyboardMetrics | null, windowHeight: number): number {
  if (frame == null) {
    return 0;
  }
  // iOS: an undocked, split or floating keyboard stops short of the window
  // bottom. Android: a floating IME reserves no inset, so RN reports a height
  // of zero or less.
  const docked =
    Platform.OS === 'ios'
      ? frame.screenY + frame.height >= windowHeight - 1
      : frame.height > 0;
  if (!docked) {
    return 0;
  }
  return Math.min(windowHeight, Math.max(0, windowHeight - frame.screenY));
}

/**
 * How much of a window-high overlay the software keyboard hides: the window
 * height minus the keyboard's top edge, both in window coordinates. A hardware
 * keyboard parks its frame below the window and covers nothing.
 *
 * iOS listens to keyboardWillChangeFrame / keyboardWillHide and the next
 * layout follows the keyboard's duration and curve. Android only has
 * keyboardDidShow / keyboardDidHide without timing, so the change is
 * immediate. Edge-to-edge windows do not shrink for the IME, which is why the
 * overlay has to move itself.
 */
export function useKeyboardOverlap(enabled = true): number {
  const { height } = useWindowDimensions();
  const [frame, setFrame] = useState<KeyboardMetrics | null>(() =>
    Keyboard.isVisible() ? Keyboard.metrics() ?? null : null,
  );
  const windowHeight = useRef(height);
  useEffect(() => {
    windowHeight.current = height;
  }, [height]);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    let current = Keyboard.isVisible() ? Keyboard.metrics() ?? null : null;
    setFrame(current);
    const update = (event: KeyboardEvent, next: KeyboardMetrics | null) => {
      const h = windowHeight.current;
      if (covered(current, h) !== covered(next, h)) {
        Keyboard.scheduleLayoutAnimation(event);
      }
      current = next;
      setFrame(next);
    };
    const subscriptions =
      Platform.OS === 'ios'
        ? [
            Keyboard.addListener('keyboardWillChangeFrame', event =>
              update(event, event.endCoordinates),
            ),
            Keyboard.addListener('keyboardWillHide', event =>
              update(event, null),
            ),
          ]
        : [
            Keyboard.addListener('keyboardDidShow', event =>
              update(event, event.endCoordinates),
            ),
            Keyboard.addListener('keyboardDidHide', event =>
              update(event, null),
            ),
          ];
    return () => subscriptions.forEach(subscription => subscription.remove());
  }, [enabled]);

  return enabled ? covered(frame, height) : 0;
}
