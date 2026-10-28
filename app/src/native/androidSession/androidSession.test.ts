import type { Spec } from '../specs/NativeRsAndroidSession';
import {
  isInPip,
  setActiveSessions,
  setPipEligible,
  setTileTarget,
  subscribeNotificationAction,
  subscribePipChange,
  subscribeTileConnect,
  takePendingTileConnect,
} from './androidSession';

let mockNativeModule: Spec | null = null;

jest.mock('../specs/NativeRsAndroidSession', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  mockNativeModule = null;
});

it('keeps every wrapper safe when the native module is absent', () => {
  expect(() => setActiveSessions([])).not.toThrow();
  expect(() => setPipEligible(true, 16, 9)).not.toThrow();
  expect(() => setTileTarget('host', 'Host')).not.toThrow();
  expect(() => subscribeNotificationAction(jest.fn())).not.toThrow();
  expect(() => subscribePipChange(jest.fn())).not.toThrow();
  expect(() => subscribeTileConnect(jest.fn())).not.toThrow();
  expect(isInPip()).toBe(false);
  expect(takePendingTileConnect()).toBe('');
});

it('forwards calls and removes event subscriptions', () => {
  const remove = jest.fn();
  const onNotificationAction = jest.fn(() => ({ remove }));
  const onPipChange = jest.fn(() => ({ remove }));
  const onTileConnect = jest.fn(() => ({ remove }));
  mockNativeModule = {
    setActiveSessions: jest.fn(),
    setPipEligible: jest.fn(),
    isInPip: jest.fn(() => true),
    setTileTarget: jest.fn(),
    takePendingTileConnect: jest.fn(() => 'host-1'),
    onNotificationAction,
    onPipChange,
    onTileConnect,
  };

  setActiveSessions([]);
  setPipEligible(true, 4, 3);
  setTileTarget('host-1', 'Host');
  expect(isInPip()).toBe(true);
  expect(takePendingTileConnect()).toBe('host-1');
  subscribeNotificationAction(jest.fn())();
  subscribePipChange(jest.fn())();
  subscribeTileConnect(jest.fn())();
  expect(mockNativeModule.setActiveSessions).toHaveBeenCalledWith([]);
  expect(mockNativeModule.setPipEligible).toHaveBeenCalledWith(true, 4, 3);
  expect(mockNativeModule.setTileTarget).toHaveBeenCalledWith('host-1', 'Host');
  expect(onNotificationAction).toHaveBeenCalled();
  expect(onPipChange).toHaveBeenCalled();
  expect(onTileConnect).toHaveBeenCalled();
  expect(remove).toHaveBeenCalledTimes(3);
});
