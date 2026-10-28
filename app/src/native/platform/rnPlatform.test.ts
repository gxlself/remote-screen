import { Linking, Platform, Share } from 'react-native';
import { createRnPlatform } from './rnPlatform';

const mockBiometryType = jest.fn<string, []>();
const mockCanAuthenticate = jest.fn<
  { biometrics: boolean; passcode: boolean },
  []
>();

jest.mock('../localAuth/localAuth', () => ({
  ...jest.requireActual<typeof import('../localAuth/localAuth')>(
    '../localAuth/localAuth',
  ),
  biometryType: () => mockBiometryType(),
  canAuthenticate: () => mockCanAuthenticate(),
}));

afterEach(() => {
  jest.restoreAllMocks();
});

beforeEach(() => {
  mockBiometryType.mockReset();
  mockCanAuthenticate.mockReset();
});

it('opens a URL using Linking and reports success', async () => {
  const openURL = jest.spyOn(Linking, 'openURL').mockResolvedValue(undefined);
  await expect(createRnPlatform().openUrl('vnc://100.64.0.7')).resolves.toBe(
    true,
  );
  expect(openURL).toHaveBeenCalledWith('vnc://100.64.0.7');
});

it('reports failure when Linking cannot open a URL', async () => {
  jest.spyOn(Linking, 'openURL').mockRejectedValue(new Error('unavailable'));
  await expect(createRnPlatform().openUrl('vnc://100.64.0.7')).resolves.toBe(
    false,
  );
});

it('delegates URL availability to Linking', async () => {
  const canOpenURL = jest.spyOn(Linking, 'canOpenURL').mockResolvedValue(false);
  await expect(createRnPlatform().canOpenUrl('vnc://100.64.0.7')).resolves.toBe(
    false,
  );
  expect(canOpenURL).toHaveBeenCalledWith('vnc://100.64.0.7');
});

it('shares a URL through the system share sheet', async () => {
  const share = jest
    .spyOn(Share, 'share')
    .mockResolvedValue({ action: Share.sharedAction, activityType: undefined });
  const url = 'https://hs.example.com/logs.zip';
  await createRnPlatform().share({ url });
  expect(share).toHaveBeenCalledWith({ url });
});

it('forwards share text and title', async () => {
  const share = jest
    .spyOn(Share, 'share')
    .mockResolvedValue({ action: Share.sharedAction, activityType: undefined });
  await createRnPlatform().share({ text: 'example public key', title: 'Key' });
  expect(share).toHaveBeenCalledWith({
    message: 'example public key',
    title: 'Key',
  });
});

it('reports authentication as unavailable until the native module is injected', async () => {
  await expect(createRnPlatform().authenticate('App lock')).resolves.toBe(
    'unavailable',
  );
});

it.each([
  ['faceID', 'faceId'],
  ['touchID', 'touchId'],
  ['opticID', 'opticId'],
  ['fingerprint', 'fingerprint'],
  ['face', 'face'],
  ['iris', 'iris'],
  ['biometric', 'biometric'],
] as const)(
  'reports %s biometrics as %s when enrolled and available',
  (nativeType, biometry) => {
    mockCanAuthenticate.mockReturnValue({ biometrics: true, passcode: true });
    mockBiometryType.mockReturnValue(nativeType);

    expect(createRnPlatform().authCapability()).toEqual({
      biometry,
      passcode: true,
    });
  },
);

it('reports no biometry when only the passcode is available', () => {
  mockCanAuthenticate.mockReturnValue({ biometrics: false, passcode: true });
  mockBiometryType.mockReturnValue('faceID');

  expect(createRnPlatform().authCapability()).toEqual({
    biometry: 'none',
    passcode: true,
  });
  expect(mockBiometryType).not.toHaveBeenCalled();
});

it('reports unknown native biometry as none', () => {
  mockCanAuthenticate.mockReturnValue({ biometrics: true, passcode: true });
  mockBiometryType.mockReturnValue('something-new');

  expect(createRnPlatform().authCapability()).toEqual({
    biometry: 'none',
    passcode: true,
  });
});

it('reports neither biometry nor passcode when authentication is unavailable', () => {
  mockCanAuthenticate.mockReturnValue({ biometrics: false, passcode: false });

  expect(createRnPlatform().authCapability()).toEqual({
    biometry: 'none',
    passcode: false,
  });
});

it('keeps clipboard, haptics and permission placeholders inert', async () => {
  const platform = createRnPlatform();
  await expect(platform.copyText('example')).resolves.toBeUndefined();
  await expect(platform.readText()).resolves.toBeNull();
  expect(platform.haptic('light')).toBeUndefined();
  await expect(platform.requestNotifications()).resolves.toBe('denied');
  await expect(platform.requestLocalNetwork()).resolves.toBe('unknown');
});

it('reads device information from React Native with the placeholder app version', () => {
  const isPad = Platform.OS === 'ios' && Platform.isPad;
  expect(createRnPlatform().deviceInfo()).toEqual({
    platform: Platform.OS === 'ios' ? (isPad ? 'ipados' : 'ios') : 'android',
    isPad,
    model:
      Platform.OS === 'android'
        ? Platform.constants.Model
        : isPad
        ? 'iPad'
        : 'iPhone',
    osVersion: String(Platform.Version),
    appVersion: '0.0.0',
  });
});
