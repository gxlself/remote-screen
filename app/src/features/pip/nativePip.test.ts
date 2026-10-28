import { Platform } from 'react-native';
import type { Spec } from '../../native/specs/NativeRsPip';
import {
  isPipActive,
  pipSupported,
  setPipEligibility,
  startPip,
  subscribePipChange,
  subscribePipPlayback,
} from './nativePip';

const mockGetConstants = jest.fn<{ supported: boolean }, []>();
const mockSetEligibility = jest.fn<void, [string, boolean, boolean]>();
const mockStart = jest.fn<void, []>();
const mockIsActive = jest.fn<boolean, []>();
const mockChange = jest.fn(() => ({ remove: jest.fn() }));
const mockPlayback = jest.fn(() => ({ remove: jest.fn() }));
let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsPip', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  mockGetConstants.mockReset();
  mockGetConstants.mockReturnValue({ supported: true });
  mockSetEligibility.mockReset();
  mockStart.mockReset();
  mockIsActive.mockReset();
  mockChange.mockClear();
  mockPlayback.mockClear();
  mockNativeModule = {
    getConstants: mockGetConstants,
    setEligibility: mockSetEligibility,
    start: mockStart,
    stop: jest.fn(),
    isActive: mockIsActive,
    onPipChange: mockChange,
    onPipPlayback: mockPlayback,
  };
});

afterEach(() => {
  mockNativeModule = null;
  jest.restoreAllMocks();
});

it('is safe when the native module is absent', () => {
  mockNativeModule = null;
  expect(pipSupported()).toBe(false);
  expect(isPipActive()).toBe(false);
  expect(() => startPip()).not.toThrow();
  expect(() => subscribePipChange(jest.fn())()).not.toThrow();
  expect(() => subscribePipPlayback(jest.fn())()).not.toThrow();
});

it('forwards eligibility and caches native constants', () => {
  expect(pipSupported()).toBe(true);
  expect(pipSupported()).toBe(true);
  setPipEligibility(null, true, false);
  expect(mockSetEligibility).toHaveBeenCalledWith('', true, false);
  expect(mockGetConstants).toHaveBeenCalledTimes(1);
});

it('reports unsupported on Android even when a module is present', () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  expect(pipSupported()).toBe(false);
});
