export const dispatchCommand = jest.fn();
export const findHostInstance_DEPRECATED = jest.fn(() => null);
export const findNodeHandle = jest.fn(() => null);
export const getNodeFromInternalInstanceHandle = jest.fn(() => null);
export const getPublicInstanceFromInternalInstanceHandle = jest.fn(() => null);
export const getPublicInstanceFromRootTag = jest.fn(() => null);
export const isChildPublicInstance = jest.fn(() => false);
export const isProfilingRenderer = jest.fn(() => false);
export const sendAccessibilityEvent = jest.fn();

export const renderElement = jest.fn(() => {
  throw new Error('Use the test renderer to render components in Jest');
});
