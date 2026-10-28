import type { FilePicker } from '../files/filesApi';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { RecordingExport } from '../../native/rscore/types';
import type { Spec } from '../../native/specs/NativeRsFiles';
import { exportRecording, pathToFileUri } from './exportShare';

let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsFiles', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

type RecordingApi = NonNullable<RsCoreApi['recording']>;

const RECORDING_ID = 'abcd1234abcd1234abcd1234abcd1234';
const PATH =
  '/data/Application Support/recordings/export/recording-abcd1234.mp4';
const URI =
  'file:///data/Application%20Support/recordings/export/recording-abcd1234.mp4';
const NAME = 'recording-abcd1234.mp4';

function setup() {
  const exportMp4 = jest.fn<Promise<RecordingExport>, [string]>(async () => ({
    path: PATH,
    bytes: 4096,
    mime: 'video/mp4',
  }));
  const discardExport = jest.fn<void, [string]>();
  const saveToFiles = jest.fn<Promise<boolean>, [string, string]>(
    async () => true,
  );
  const core = createFakeCore();
  (core as RsCoreApi).recording = {
    start: jest.fn(),
    stop: jest.fn(),
    status: jest.fn(),
    list: jest.fn(),
    remove: jest.fn(),
    open: jest.fn(),
    control: jest.fn(),
    close: jest.fn(),
    exportMp4,
    discardExport,
  } as RecordingApi;
  const picker: FilePicker = {
    pickFiles: jest.fn(async () => []),
    saveToFiles,
  };
  return { core, exportMp4, discardExport, saveToFiles, picker };
}

beforeEach(() => {
  mockNativeModule = null;
});

it('saves the exported file and then discards it', async () => {
  const { core, exportMp4, discardExport, saveToFiles, picker } = setup();
  await expect(exportRecording(core, RECORDING_ID, picker)).resolves.toBe(
    'saved',
  );
  expect(exportMp4).toHaveBeenCalledWith(RECORDING_ID);
  expect(saveToFiles).toHaveBeenCalledWith(URI, NAME);
  expect(discardExport).toHaveBeenCalledTimes(1);
  expect(discardExport).toHaveBeenCalledWith(PATH);
  expect(discardExport.mock.invocationCallOrder[0]).toBeGreaterThan(
    saveToFiles.mock.invocationCallOrder[0],
  );
});

it('reports a cancelled save and still discards the file', async () => {
  const { core, discardExport, saveToFiles, picker } = setup();
  saveToFiles.mockResolvedValue(false);
  await expect(exportRecording(core, RECORDING_ID, picker)).resolves.toBe(
    'cancelled',
  );
  expect(discardExport).toHaveBeenCalledWith(PATH);
});

it('returns unsupported when the core cannot export', async () => {
  const { core, exportMp4, discardExport, saveToFiles, picker } = setup();
  exportMp4.mockRejectedValue(
    new RsCoreError('unsupported', 'recording.unsupported'),
  );
  await expect(exportRecording(core, RECORDING_ID, picker)).resolves.toBe(
    'unsupported',
  );
  expect(saveToFiles).not.toHaveBeenCalled();
  expect(discardExport).not.toHaveBeenCalled();
});

it('rethrows export errors such as busy', async () => {
  const { core, exportMp4, discardExport, saveToFiles, picker } = setup();
  const busy = new RsCoreError('http', 'recording.busy', 429);
  exportMp4.mockRejectedValue(busy);
  await expect(exportRecording(core, RECORDING_ID, picker)).rejects.toBe(busy);
  expect(saveToFiles).not.toHaveBeenCalled();
  expect(discardExport).not.toHaveBeenCalled();
});

it('returns unsupported when the picker is unavailable and discards the file', async () => {
  const { core, discardExport, saveToFiles, picker } = setup();
  saveToFiles.mockRejectedValue(new RsCoreError('unsupported', 'files'));
  await expect(exportRecording(core, RECORDING_ID, picker)).resolves.toBe(
    'unsupported',
  );
  expect(discardExport).toHaveBeenCalledWith(PATH);
});

it('rethrows other picker errors after discarding the file', async () => {
  const { core, discardExport, saveToFiles, picker } = setup();
  const failure = new Error('copy_failed');
  saveToFiles.mockRejectedValue(failure);
  await expect(exportRecording(core, RECORDING_ID, picker)).rejects.toBe(
    failure,
  );
  expect(discardExport).toHaveBeenCalledWith(PATH);
});

it('ignores a failing discard', async () => {
  const { core, discardExport, picker } = setup();
  discardExport.mockImplementation(() => {
    throw new RsCoreError('internal', 'recording.io: busy');
  });
  await expect(exportRecording(core, RECORDING_ID, picker)).resolves.toBe(
    'saved',
  );
  expect(discardExport).toHaveBeenCalledWith(PATH);
});

it('returns unsupported without the recording api', async () => {
  const { core, saveToFiles, picker } = setup();
  (core as RsCoreApi).recording = undefined;
  await expect(exportRecording(core, RECORDING_ID, picker)).resolves.toBe(
    'unsupported',
  );
  expect(saveToFiles).not.toHaveBeenCalled();
});

it('uses the native picker by default', async () => {
  const { core, discardExport } = setup();
  const nativeSave = jest.fn<Promise<boolean>, [string, string]>(
    async () => true,
  );
  mockNativeModule = {
    getConstants: () => ({ filesDir: '/files', inboxDir: '/inbox' }),
    pickFiles: jest.fn(async () => []),
    saveToFiles: nativeSave,
    importIncoming: jest.fn(async () => null),
    setTransfersActive: jest.fn(),
  };
  await expect(exportRecording(core, RECORDING_ID)).resolves.toBe('saved');
  expect(nativeSave).toHaveBeenCalledWith(URI, NAME);
  expect(discardExport).toHaveBeenCalledWith(PATH);

  mockNativeModule = null;
  await expect(exportRecording(core, RECORDING_ID)).resolves.toBe(
    'unsupported',
  );
});

it('encodes each path segment of the file uri', () => {
  expect(pathToFileUri('/a b/c#d/50%/é?.mp4')).toBe(
    'file:///a%20b/c%23d/50%25/%C3%A9%3F.mp4',
  );
  expect(pathToFileUri(PATH)).toBe(URI);
});
