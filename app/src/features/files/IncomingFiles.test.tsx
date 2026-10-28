import { act, cleanup, waitFor } from '@testing-library/react-native';
import { Linking } from 'react-native';
import IncomingFiles from './IncomingFiles';
import { useIncomingShare } from './incomingShare';
import IncomingPairLinks from '../pairing/IncomingPairLinks';
import { renderWithProviders } from '../../test/render';

let urlListener: ((event: { url: string }) => void) | undefined;
let urlListeners: Array<(event: { url: string }) => void> = [];
let navigationReadyListener: (() => void) | undefined;
let navigationStateListener: (() => void) | undefined;

jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    addListener: jest.fn((event: string, listener: () => void) => {
      if (event === 'ready') {
        navigationReadyListener = listener;
      }
      if (event === 'state') {
        navigationStateListener = listener;
      }
      return jest.fn();
    }),
    isReady: jest.fn(() => true),
    navigate: jest.fn(),
  },
}));

jest.mock('../../native/specs/NativeRsDevLog', () => ({
  __esModule: true,
  default: { log: jest.fn() },
}));

jest.mock('../../native/specs/NativeRsFiles', () => ({
  __esModule: true,
  default: {
    importIncoming: jest.fn(),
  },
}));

const navigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as {
  addListener: jest.Mock;
  isReady: jest.Mock;
  navigate: jest.Mock;
};
const nativeFiles = jest.requireMock('../../native/specs/NativeRsFiles')
  .default as {
  importIncoming: jest.Mock;
};
const nativeLog = jest.requireMock('../../native/specs/NativeRsDevLog').default
  .log as jest.Mock;
const getInitialURL = jest.spyOn(Linking, 'getInitialURL');
const addEventListener = jest.spyOn(Linking, 'addEventListener');

async function settle() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  useIncomingShare.setState({ pending: [] });
  urlListener = undefined;
  urlListeners = [];
  navigationReadyListener = undefined;
  navigationStateListener = undefined;
  navigationRef.addListener.mockClear();
  navigationRef.isReady.mockReset();
  navigationRef.isReady.mockReturnValue(true);
  navigationRef.navigate.mockReset();
  nativeFiles.importIncoming.mockReset();
  getInitialURL.mockReset();
  getInitialURL.mockResolvedValue(null);
  addEventListener.mockReset();
  addEventListener.mockImplementation((_event, listener) => {
    urlListener = listener;
    urlListeners.push(listener);
    return { remove: jest.fn() };
  });
});

afterEach(async () => {
  await cleanup();
  jest.clearAllMocks();
});

it('imports a cold-start file and navigates to Files', async () => {
  const url = 'file:///tmp/a.pdf';
  getInitialURL.mockResolvedValue(url);
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/a.pdf',
    name: 'a.pdf',
    size: 12,
  });

  await renderWithProviders(<IncomingFiles />);
  await settle();

  await waitFor(() =>
    expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1),
  );
  expect(nativeFiles.importIncoming).toHaveBeenCalledWith(url);
  expect(useIncomingShare.getState().pending).toEqual([
    { uri: 'file:///tmp/RsInbox/a.pdf', name: 'a.pdf', size: 12 },
  ]);
  await waitFor(() =>
    expect(navigationRef.navigate).toHaveBeenCalledWith('MainTabs', {
      screen: 'Files',
    }),
  );
});

it('deduplicates initial and runtime delivery of the same URL', async () => {
  const url = 'file:///tmp/a.pdf';
  getInitialURL.mockResolvedValue(url);
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/a.pdf',
    name: 'a.pdf',
    size: 12,
  });

  await renderWithProviders(<IncomingFiles />);
  await settle();
  await act(async () => urlListener?.({ url }));
  await settle();

  expect(getInitialURL).toHaveBeenCalledTimes(1);
  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
});

it('imports a content URI once and opens an imported pairing file', async () => {
  const url =
    'content://com.android.providers.downloads.documents/document/123';
  getInitialURL.mockResolvedValue(url);
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/Studio.rspair',
    name: 'Studio.rspair',
    size: 128,
  });

  await renderWithProviders(
    <>
      <IncomingPairLinks />
      <IncomingFiles />
    </>,
  );
  await settle();

  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
  expect(nativeFiles.importIncoming).toHaveBeenCalledWith(url);
  expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
    uri: 'file:///tmp/RsInbox/Studio.rspair',
  });
  expect(useIncomingShare.getState().pending).toEqual([]);

  await act(async () => {
    for (const listener of urlListeners) {
      listener({ url });
    }
  });
  await settle();
  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
});

it('queues an imported content URI with a regular file name', async () => {
  const url =
    'content://com.android.providers.downloads.documents/document/456';
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/photo.jpg',
    name: 'photo.jpg',
    size: 256,
  });

  await renderWithProviders(<IncomingFiles />);
  await settle();
  await act(async () => urlListener?.({ url }));
  await settle();

  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
  expect(useIncomingShare.getState().pending).toEqual([
    {
      uri: 'file:///tmp/RsInbox/photo.jpg',
      name: 'photo.jpg',
      size: 256,
    },
  ]);
  expect(navigationRef.navigate).toHaveBeenCalledWith('MainTabs', {
    screen: 'Files',
  });
});

it('logs and drops a content URI when native import returns null', async () => {
  const url =
    'content://com.android.providers.downloads.documents/document/789';
  nativeFiles.importIncoming.mockResolvedValue(null);

  await renderWithProviders(<IncomingFiles />);
  await act(async () => urlListener?.({ url }));
  await settle();

  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
  expect(nativeLog).toHaveBeenCalledWith('RS-PAIRFILE-INCOMING {"ok":false}');
  expect(useIncomingShare.getState().pending).toEqual([]);
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

it('logs and drops a content URI when native import rejects', async () => {
  const url =
    'content://com.android.providers.downloads.documents/document/999';
  nativeFiles.importIncoming.mockRejectedValue(new Error('provider failed'));

  await renderWithProviders(<IncomingFiles />);
  await act(async () => urlListener?.({ url }));
  await settle();

  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
  expect(nativeLog).toHaveBeenCalledWith('RS-PAIRFILE-INCOMING {"ok":false}');
  expect(useIncomingShare.getState().pending).toEqual([]);
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

it('deduplicates a content URI delivered at cold start and at runtime', async () => {
  const url =
    'content://com.android.providers.downloads.documents/document/1000';
  getInitialURL.mockResolvedValue(url);
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/photo.jpg',
    name: 'photo.jpg',
    size: 256,
  });

  await renderWithProviders(<IncomingFiles />);
  await settle();
  await act(async () => urlListener?.({ url }));
  await settle();

  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
});

it('leaves pairing links for the pairing handlers', async () => {
  getInitialURL.mockResolvedValue('rsp://pair?v=1');
  await renderWithProviders(<IncomingFiles />);
  await settle();
  expect(getInitialURL).toHaveBeenCalledTimes(1);
  await act(async () => urlListener?.({ url: 'file:///tmp/studio.RSPAIR' }));

  expect(nativeFiles.importIncoming).not.toHaveBeenCalled();
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

it('does not queue or navigate when native import returns null', async () => {
  getInitialURL.mockResolvedValue('file:///tmp/a.pdf');
  nativeFiles.importIncoming.mockResolvedValue(null);
  await renderWithProviders(<IncomingFiles />);
  await settle();

  expect(useIncomingShare.getState().pending).toEqual([]);
  expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

it('waits for navigation readiness after importing', async () => {
  navigationRef.isReady.mockReturnValue(false);
  getInitialURL.mockResolvedValue('file:///tmp/a.pdf');
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/a.pdf',
    name: 'a.pdf',
    size: 12,
  });

  await renderWithProviders(<IncomingFiles />);
  await settle();
  expect(navigationRef.navigate).not.toHaveBeenCalled();

  navigationRef.isReady.mockReturnValue(true);
  await act(async () => navigationReadyListener?.());
  await act(async () => navigationStateListener?.());
  expect(navigationRef.navigate).toHaveBeenCalledWith('MainTabs', {
    screen: 'Files',
  });
  expect(navigationRef.navigate).toHaveBeenCalledTimes(1);
});

it('handles the same URL again after the ten-second deduplication window', async () => {
  const now = jest.spyOn(Date, 'now').mockReturnValue(1_000);
  const url = 'file:///tmp/a.pdf';
  getInitialURL.mockResolvedValue(url);
  nativeFiles.importIncoming.mockResolvedValue({
    uri: 'file:///tmp/RsInbox/a.pdf',
    name: 'a.pdf',
    size: -1,
  });
  try {
    await renderWithProviders(<IncomingFiles />);
    await settle();
    now.mockReturnValue(10_999);
    await act(async () => urlListener?.({ url }));
    expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);

    now.mockReturnValue(11_000);
    await act(async () => urlListener?.({ url }));
    await settle();
    expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(2);
    expect(useIncomingShare.getState().pending).toHaveLength(2);
    expect(useIncomingShare.getState().pending[0]?.size).toBeNull();
  } finally {
    now.mockRestore();
  }
});

it('logs import failures without showing a UI or logging the shared URL', async () => {
  const url = 'file:///tmp/private-report.pdf';
  getInitialURL.mockResolvedValue(url);
  nativeFiles.importIncoming.mockRejectedValue(new Error(`Cannot read ${url}`));
  await renderWithProviders(<IncomingFiles />);
  await settle();

  expect(nativeLog).toHaveBeenCalledWith('RS-FILES-INCOMING {"ok":false}');
  expect(useIncomingShare.getState().pending).toEqual([]);
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});
