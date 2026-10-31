import { render, screen } from '@testing-library/react-native';
import AppRoot from './AppRoot';

const mockNetInfoMount = jest.fn();
const mockLifecycleMount = jest.fn();

jest.mock('../native/netinfo/NetInfoBridge', () => ({
  __esModule: true,
  default: function MockNetInfoBridge() {
    mockNetInfoMount();
    return null;
  },
}));

jest.mock('../native/lifecycle/LifecycleBridge', () => ({
  __esModule: true,
  default: function MockLifecycleBridge() {
    mockLifecycleMount();
    return null;
  },
}));

jest.mock('./NativeBridges', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { Text } =
    jest.requireActual<typeof import('react-native')>('react-native');
  return {
    __esModule: true,
    default: function MockNativeBridges() {
      return React.createElement(Text, { testID: 'native-bridges' });
    },
  };
});

const NativeBridges =
  jest.requireActual<typeof import('./NativeBridges')>(
    './NativeBridges',
  ).default;

beforeEach(() => {
  mockNetInfoMount.mockClear();
  mockLifecycleMount.mockClear();
});

it('mounts each native bridge once', async () => {
  await render(<NativeBridges />);

  expect(mockNetInfoMount).toHaveBeenCalledTimes(1);
});

it('mounts LifecycleBridge once', async () => {
  await render(<NativeBridges />);

  expect(mockLifecycleMount).toHaveBeenCalledTimes(1);
});

it('mounts NativeBridges from AppRoot', async () => {
  await render(<AppRoot />);

  expect(screen.getByTestId('native-bridges')).toBeOnTheScreen();
});
