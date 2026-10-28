import type { Spec } from '../../native/specs/NativeRsFiles';
import { isUnsupported } from './filesApi';
import { createNativePicker } from './nativePicker';

const mockGetConstants = jest.fn<{ filesDir: string; inboxDir: string }, []>();
const mockPickFiles = jest.fn<
  Promise<{ uri: string; name: string; size: number }[]>,
  []
>();
const mockSaveToFiles = jest.fn<Promise<boolean>, [string, string]>();
const mockImportIncoming = jest.fn<
  Promise<{ uri: string; name: string; size: number } | null>,
  [string]
>();
const mockSetTransfersActive = jest.fn<void, [boolean]>();

let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsFiles', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

beforeEach(() => {
  mockGetConstants.mockReturnValue({
    filesDir: '/files',
    inboxDir: '/inbox',
  });
  mockPickFiles.mockReset();
  mockSaveToFiles.mockReset();
  mockImportIncoming.mockReset();
  mockSetTransfersActive.mockReset();
  mockNativeModule = {
    getConstants: mockGetConstants,
    pickFiles: mockPickFiles,
    saveToFiles: mockSaveToFiles,
    importIncoming: mockImportIncoming,
    setTransfersActive: mockSetTransfersActive,
  };
});

it('falls back to the unsupported picker when the module is absent', async () => {
  mockNativeModule = null;
  const picker = createNativePicker();
  const error = await picker.pickFiles().catch(caught => caught);
  expect(isUnsupported(error)).toBe(true);
});

it('maps unknown native sizes to null and returns size in bytes', async () => {
  mockPickFiles.mockResolvedValue([
    { uri: 'file:///inbox/a%20b.pdf', name: 'a b.pdf', size: -1 },
    { uri: 'file:///inbox/c.png', name: 'c.png', size: 120 },
  ]);
  const picker = createNativePicker();
  await expect(picker.pickFiles()).resolves.toEqual([
    { uri: 'file:///inbox/a%20b.pdf', name: 'a b.pdf', size: null },
    { uri: 'file:///inbox/c.png', name: 'c.png', size: 120 },
  ]);
  expect(mockPickFiles).toHaveBeenCalledTimes(1);
});

it('passes saveToFiles through and returns the native result', async () => {
  mockSaveToFiles.mockResolvedValue(true);
  const picker = createNativePicker();
  await expect(
    picker.saveToFiles('file:///inbox/report.pdf', 'report.pdf'),
  ).resolves.toBe(true);
  expect(mockSaveToFiles).toHaveBeenCalledWith(
    'file:///inbox/report.pdf',
    'report.pdf',
  );
});
