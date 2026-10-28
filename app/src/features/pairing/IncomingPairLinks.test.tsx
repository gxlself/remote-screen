import { act, cleanup } from '@testing-library/react-native';
import { Linking } from 'react-native';
import IncomingPairLinks from './IncomingPairLinks';
import { renderWithProviders } from '../../test/render';

type UrlListener = (event: { url: string }) => void;

let urlListener: UrlListener | undefined;
let stateListener: (() => void) | undefined;
const removeUrlListener = jest.fn();
const removeStateListener = jest.fn();

jest.mock('../../app/navigation/navigationRef', () => {
  const navigationRef = {
    addListener: jest.fn(() => jest.fn()),
    isReady: jest.fn(() => true),
    navigate: jest.fn(),
  };
  return { navigationRef };
});

jest.mock('../../native/specs/NativeRsFiles', () => ({
  __esModule: true,
  default: { importIncoming: jest.fn() },
}));

jest.mock('../../native/specs/NativeRsDevLog', () => ({
  __esModule: true,
  default: { log: jest.fn() },
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
  jest.useFakeTimers();
  jest.setSystemTime(1_792_465_134_000);
  urlListener = undefined;
  stateListener = undefined;
  navigationRef.addListener.mockReset();
  navigationRef.addListener.mockImplementation(
    (event: string, listener: () => void) => {
      if (event === 'state') {
        stateListener = listener;
      }
      return removeStateListener;
    },
  );
  navigationRef.navigate.mockReset();
  navigationRef.isReady.mockReset();
  navigationRef.isReady.mockReturnValue(true);
  nativeFiles.importIncoming.mockReset();
  nativeLog.mockReset();
  getInitialURL.mockReset();
  getInitialURL.mockResolvedValue(null);
  addEventListener.mockReset();
  addEventListener.mockImplementation((_event, listener) => {
    urlListener = listener;
    return { remove: removeUrlListener };
  });
  removeUrlListener.mockClear();
  removeStateListener.mockClear();
});

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
  jest.restoreAllMocks();
});

describe('IncomingPairLinks', () => {
  it('opens a cold-start pairing-file link once', async () => {
    const url = 'rsp://pairfile?v=1&h=h1&s=secret';
    getInitialURL.mockResolvedValue(url);

    const view = await renderWithProviders(<IncomingPairLinks />);
    await settle();

    expect(getInitialURL).toHaveBeenCalledTimes(1);
    expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
      text: url,
    });
    await view.unmount();
    expect(removeUrlListener).toHaveBeenCalled();
    expect(removeStateListener).toHaveBeenCalled();
  });

  it('opens a runtime .rspair file and ignores unrelated links', async () => {
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    const file = 'file:///private/var/mobile/Inbox/a.rspair';
    const copied = 'file:///tmp/RsInbox/a.rspair';
    nativeFiles.importIncoming.mockResolvedValue({
      uri: copied,
      name: 'a.rspair',
      size: 128,
    });
    await act(async () => urlListener?.({ url: file }));
    await act(async () =>
      urlListener?.({ url: 'https://hs.example.com/a.rspair' }),
    );
    await settle();

    expect(nativeFiles.importIncoming).toHaveBeenCalledTimes(1);
    expect(nativeFiles.importIncoming).toHaveBeenCalledWith(file);
    expect(navigationRef.navigate).toHaveBeenCalledTimes(1);
    expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
      uri: copied,
    });
  });

  it('uses the original .rspair URI when native import returns null', async () => {
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    const file = 'file:///private/var/mobile/Inbox/a.rspair';
    nativeFiles.importIncoming.mockResolvedValue(null);
    await act(async () => urlListener?.({ url: file }));
    await settle();

    expect(nativeFiles.importIncoming).toHaveBeenCalledWith(file);
    expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
      uri: file,
    });
  });

  it('uses the original .rspair URI and logs when native import fails', async () => {
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    const file = 'file:///private/var/mobile/Inbox/private.rspair';
    nativeFiles.importIncoming.mockRejectedValue(
      new Error(`Cannot read ${file}`),
    );
    await act(async () => urlListener?.({ url: file }));
    await settle();

    expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
      uri: file,
    });
    expect(nativeLog).toHaveBeenCalledWith('RS-PAIRFILE-INCOMING {"ok":false}');
  });

  it('opens a deep link without importing it', async () => {
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    const url = 'rsp://pairfile?v=1&h=h1&s=secret';
    await act(async () => urlListener?.({ url }));
    await settle();

    expect(nativeFiles.importIncoming).not.toHaveBeenCalled();
    expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
      text: url,
    });
  });

  it('leaves ordinary files for IncomingFiles', async () => {
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    await act(async () => urlListener?.({ url: 'file:///tmp/a.pdf' }));
    await settle();

    expect(nativeFiles.importIncoming).not.toHaveBeenCalled();
    expect(navigationRef.navigate).not.toHaveBeenCalled();
  });

  it('leaves content URIs for IncomingFiles', async () => {
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    await act(async () =>
      urlListener?.({
        url: 'content://com.android.providers.downloads.documents/document/123',
      }),
    );
    await settle();

    expect(nativeFiles.importIncoming).not.toHaveBeenCalled();
    expect(navigationRef.navigate).not.toHaveBeenCalled();
  });

  it('waits for navigation readiness and then opens only once', async () => {
    navigationRef.isReady.mockReturnValue(false);
    const url = 'rsp://pairfile?v=1&h=h1&s=secret';
    getInitialURL.mockResolvedValue(url);
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    expect(navigationRef.navigate).not.toHaveBeenCalled();
    navigationRef.isReady.mockReturnValue(true);
    await act(async () => stateListener?.());
    await act(async () => stateListener?.());

    expect(navigationRef.navigate).toHaveBeenCalledTimes(1);
    expect(navigationRef.navigate).toHaveBeenCalledWith('PairFile', {
      text: url,
    });
  });

  it('deduplicates the same URL from initialURL and a live event', async () => {
    const url = 'rsp://pairfile?v=1&h=h1&s=secret';
    getInitialURL.mockResolvedValue(url);
    await renderWithProviders(<IncomingPairLinks />);
    await settle();

    await act(async () => urlListener?.({ url }));

    expect(navigationRef.navigate).toHaveBeenCalledTimes(1);
  });
});
