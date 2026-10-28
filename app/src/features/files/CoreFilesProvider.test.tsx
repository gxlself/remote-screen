import { act, cleanup, render, screen } from '@testing-library/react-native';
import { Text } from 'react-native';
import { CoreProvider } from '../../native/rscore/CoreContext';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { computers } from '../../native/rscore/fixtures';
import type { Spec } from '../../native/specs/NativeRsFiles';
import { CoreFilesProvider } from './CoreFilesProvider';
import { useFiles } from './FilesContext';

const mockSetTransfersActive = jest.fn<void, [boolean]>();
const mockPickFiles = jest.fn<Promise<never[]>, []>();
const mockSaveToFiles = jest.fn<Promise<boolean>, [string, string]>();
const mockImportIncoming = jest.fn<Promise<null>, [string]>();
const mockGetConstants = jest.fn<{ filesDir: string; inboxDir: string }, []>();

let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsFiles', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

const hostId = computers[0]?.hostId ?? '';

function Probe() {
  const { available } = useFiles();
  return <Text testID="files-available">{String(available)}</Text>;
}

function renderProvider(core: ReturnType<typeof createFakeCore>) {
  return render(
    <CoreProvider core={core}>
      <CoreFilesProvider>
        <Probe />
      </CoreFilesProvider>
    </CoreProvider>,
  );
}

beforeEach(() => {
  jest.useFakeTimers();
  mockSetTransfersActive.mockReset();
  mockPickFiles.mockReset();
  mockSaveToFiles.mockReset();
  mockImportIncoming.mockReset();
  mockGetConstants.mockReset();
  mockGetConstants.mockReturnValue({ filesDir: '/files', inboxDir: '/inbox' });
  mockNativeModule = {
    getConstants: mockGetConstants,
    pickFiles: mockPickFiles,
    saveToFiles: mockSaveToFiles,
    importIncoming: mockImportIncoming,
    setTransfersActive: mockSetTransfersActive,
  };
});

afterEach(() => {
  cleanup();
  jest.clearAllTimers();
  jest.useRealTimers();
});

it('provides an available files service through the context', async () => {
  const core = createFakeCore();
  await renderProvider(core);
  expect(screen.getByTestId('files-available')).toHaveTextContent('true');
});

it('tracks transfers becoming active and inactive exactly once', async () => {
  const core = createFakeCore();
  const view = await renderProvider(core);
  mockSetTransfersActive.mockClear();

  await act(async () => {
    await core.files.download(hostId, '/Users/demo/notes-1020.md');
  });
  expect(mockSetTransfersActive.mock.calls).toEqual([[true]]);

  for (let index = 0; index < 12; index += 1) {
    await act(async () => {
      await jest.advanceTimersByTimeAsync(100);
    });
  }

  expect(mockSetTransfersActive.mock.calls).toEqual([[true], [false]]);
  await view.unmount();
});
