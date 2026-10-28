import {
  authenticate,
  biometryType,
  canAuthenticate,
  setPrivacyCover,
} from './localAuth';

const mockAuthenticate = jest.fn<Promise<string>, [string, string]>();
const mockGetBiometryType = jest.fn<string, []>();
const mockCanAuthenticate = jest.fn<
  { biometrics: boolean; passcode: boolean },
  []
>();
const mockSetPrivacyCover = jest.fn<void, [boolean]>();

let mockNativeModule: {
  authenticate: typeof mockAuthenticate;
  getBiometryType: typeof mockGetBiometryType;
  canAuthenticate: typeof mockCanAuthenticate;
  setPrivacyCover: typeof mockSetPrivacyCover;
} | null = null;

jest.mock('../specs/NativeLocalAuth', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  mockAuthenticate.mockReset();
  mockGetBiometryType.mockReset();
  mockCanAuthenticate.mockReset();
  mockSetPrivacyCover.mockReset();
  mockNativeModule = {
    authenticate: mockAuthenticate,
    getBiometryType: mockGetBiometryType,
    canAuthenticate: mockCanAuthenticate,
    setPrivacyCover: mockSetPrivacyCover,
  };
});

it('passes the device passcode fallback title and accepts ok', async () => {
  mockAuthenticate.mockResolvedValue('ok');
  await expect(authenticate('解锁 Remote Screen')).resolves.toBe('ok');
  expect(mockAuthenticate).toHaveBeenCalledWith(
    '解锁 Remote Screen',
    '用设备密码',
  );
});

it('normalizes unexpected native results to failed', async () => {
  mockAuthenticate.mockResolvedValue('weird');
  await expect(authenticate('reason')).resolves.toBe('failed');
});

it('returns unavailable when the native module is missing', async () => {
  mockNativeModule = null;
  await expect(authenticate('reason')).resolves.toBe('unavailable');
  expect(biometryType()).toBe('none');
  expect(canAuthenticate()).toEqual({ biometrics: false, passcode: false });
});

it('forwards capability and privacy cover calls', () => {
  mockGetBiometryType.mockReturnValue('faceID');
  mockCanAuthenticate.mockReturnValue({ biometrics: true, passcode: true });
  expect(biometryType()).toBe('faceID');
  expect(canAuthenticate()).toEqual({ biometrics: true, passcode: true });

  setPrivacyCover(true);
  expect(mockSetPrivacyCover).toHaveBeenCalledWith(true);
});
