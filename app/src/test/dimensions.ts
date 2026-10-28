import { Dimensions, type useWindowDimensions } from 'react-native';

export const PHONE_WINDOW = { width: 390, height: 844 };

// The public RN getter is not writable; spy on its underlying default export.
const windowDimensionsModule =
  // eslint-disable-next-line @react-native/no-deep-imports
  require('react-native/Libraries/Utilities/useWindowDimensions') as {
    default: typeof useWindowDimensions;
  };

/** Override the hook (not the global Dimensions event stream) for one test. */
export function mockWindowDimensions(d: {
  width: number;
  height: number;
}): () => void {
  const spy = jest
    .spyOn(windowDimensionsModule, 'default')
    .mockReturnValue({ ...Dimensions.get('window'), ...d });
  return () => spy.mockRestore();
}

/** Render one test at the narrow-phone layout size. */
export function mockPhoneDimensions(): () => void {
  return mockWindowDimensions(PHONE_WINDOW);
}
