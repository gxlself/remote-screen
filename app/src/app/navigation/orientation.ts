import { useEffect, useState } from 'react';
import { Dimensions, Platform } from 'react-native';
import type { ScaledSize } from 'react-native';

// Android's tablet breakpoint: a shortest screen side of 600 dp.
export const TABLET_MIN_SIDE = 600;

export function isTablet(
  screen: Pick<ScaledSize, 'width' | 'height'>,
): boolean {
  if (Platform.OS === 'ios') {
    return Platform.isPad;
  }
  return Math.min(screen.width, screen.height) >= TABLET_MIN_SIDE;
}

/** Tablets are never locked; a foldable is re-checked when it opens or closes. */
export function useIsTablet(): boolean {
  const [tablet, setTablet] = useState(() =>
    isTablet(Dimensions.get('screen')),
  );
  useEffect(() => {
    const onChange = ({ screen }: { screen: ScaledSize }) =>
      setTablet(isTablet(screen));
    const subscription = Dimensions.addEventListener('change', onChange);
    return () => subscription.remove();
  }, []);
  return tablet;
}
