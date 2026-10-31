import {
  appVersion,
  clipboardChangeCount,
  copyText,
  haptic,
  maxFps,
  modelId,
  readText,
  requestLocalNetwork,
  requestNotifications,
} from './system';

const mockCopyText = jest.fn<void, [string]>();
const mockReadText = jest.fn<Promise<string | null>, []>();
const mockHaptic = jest.fn<void, [string]>();
const mockNotificationStatus = jest.fn<Promise<string>, []>();
const mockRequestNotifications = jest.fn<Promise<string>, []>();
const mockRequestLocalNetwork = jest.fn<Promise<string>, [number]>();
const mockGetConstants = jest.fn(() => ({
  appVersion: '1.2.3',
  buildNumber: '42',
  bundleId: 'ai.gxlself.remotescreen',
  modelId: 'iPhone16,2',
  maxFps: 120,
}));

let mockNativeModule: {
  getConstants: typeof mockGetConstants;
  copyText: typeof mockCopyText;
  hasText: jest.Mock<boolean, []>;
  clipboardChangeCount: jest.Mock<number, []>;
  readText: typeof mockReadText;
  haptic: typeof mockHaptic;
  notificationStatus: typeof mockNotificationStatus;
  requestNotifications: typeof mockRequestNotifications;
  requestLocalNetwork: typeof mockRequestLocalNetwork;
} | null = null;

jest.mock('../../specs/NativeRsSystem', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  mockCopyText.mockReset();
  mockReadText.mockReset();
  mockHaptic.mockReset();
  mockNotificationStatus.mockReset();
  mockRequestNotifications.mockReset();
  mockRequestLocalNetwork.mockReset();
  mockGetConstants.mockClear();
  mockNativeModule = {
    getConstants: mockGetConstants,
    copyText: mockCopyText,
    hasText: jest.fn(),
    clipboardChangeCount: jest.fn(() => 7),
    readText: mockReadText,
    haptic: mockHaptic,
    notificationStatus: mockNotificationStatus,
    requestNotifications: mockRequestNotifications,
    requestLocalNetwork: mockRequestLocalNetwork,
  };
});

it('forwards clipboard values, including null', async () => {
  mockReadText.mockResolvedValueOnce(null).mockResolvedValueOnce('hello');
  await expect(readText()).resolves.toBeNull();
  await expect(readText()).resolves.toBe('hello');
  await copyText('hello');
  expect(mockCopyText).toHaveBeenCalledWith('hello');
});

it('reports the clipboard change count, null when the platform has none', () => {
  expect(clipboardChangeCount()).toBe(7);
  mockNativeModule?.clipboardChangeCount.mockReturnValue(-1);
  expect(clipboardChangeCount()).toBeNull();
});

it('uses the 30 second local network timeout and normalizes results', async () => {
  mockRequestLocalNetwork.mockResolvedValue('x');
  await expect(requestLocalNetwork()).resolves.toBe('unknown');
  expect(mockRequestLocalNetwork).toHaveBeenCalledWith(30000);
  mockRequestLocalNetwork.mockResolvedValue('granted');
  await expect(requestLocalNetwork()).resolves.toBe('granted');
});

it('normalizes notification results', async () => {
  mockRequestNotifications.mockResolvedValue('granted');
  await expect(requestNotifications()).resolves.toBe('granted');
  mockRequestNotifications.mockResolvedValue('notDetermined');
  await expect(requestNotifications()).resolves.toBe('denied');
});

it('reads constants and exposes display information', () => {
  expect(appVersion()).toBe('1.2.3');
  expect(maxFps()).toBe(120);
  expect(modelId()).toBe('iPhone16,2');
});

it('uses the P0 placeholders when the native module is missing', async () => {
  mockNativeModule = null;
  await expect(copyText('hello')).resolves.toBeUndefined();
  await expect(readText()).resolves.toBeNull();
  expect(clipboardChangeCount()).toBeNull();
  expect(haptic('light')).toBeUndefined();
  await expect(requestNotifications()).resolves.toBe('denied');
  await expect(requestLocalNetwork()).resolves.toBe('unknown');
  expect(appVersion()).toBe('0.0.0');
  expect(maxFps()).toBe(60);
  expect(modelId()).toBe('unknown');
});
