import { act, cleanup, screen } from '@testing-library/react-native';
import { Alert, Linking } from 'react-native';
import DeepLinkPairing from './DeepLinkPairing';
import { useLockState } from '../lock/lockState';
import { pairQrStudio } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';

let mockUrlListener: ((event: { url: string }) => void) | undefined;
const mockNavigationListeners = new Map<string, () => void>();

jest.mock('../../app/navigation/navigationRef', () => {
  const navigationRef = {
    addListener: jest.fn(() => jest.fn()),
    dispatch: jest.fn(),
    getCurrentRoute: jest.fn(() => ({ name: 'MainTabs' })),
    isReady: jest.fn(() => true),
    navigate: jest.fn(),
  };
  return { navigationRef };
});

const mockNavigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as {
  addListener: jest.Mock;
  dispatch: jest.Mock;
  getCurrentRoute: jest.Mock;
  isReady: jest.Mock;
  navigate: jest.Mock;
};

const mockAlert = jest.spyOn(Alert, 'alert');
const mockGetInitialURL = jest.spyOn(Linking, 'getInitialURL');
const mockAddEventListener = jest.spyOn(Linking, 'addEventListener');

async function settle() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  mockAlert.mockReset();
  mockGetInitialURL.mockResolvedValue(null);
  mockAddEventListener.mockImplementation((_event, listener) => {
    mockUrlListener = listener;
    return { remove: jest.fn() };
  });
  mockNavigationListeners.clear();
  mockNavigationRef.addListener.mockImplementation((event, listener) => {
    mockNavigationListeners.set(event, listener);
    return () => mockNavigationListeners.delete(event);
  });
  mockNavigationRef.dispatch.mockClear();
  mockNavigationRef.getCurrentRoute.mockReturnValue({ name: 'MainTabs' });
  mockNavigationRef.navigate.mockClear();
  mockNavigationRef.isReady.mockReturnValue(true);
  mockUrlListener = undefined;
  useLockState.setState({ locked: false });
});

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
});

function alertButtons() {
  return mockAlert.mock.calls[0]?.[2] as Array<{
    text?: string;
    onPress?: () => void;
  }>;
}

it('confirms a valid cold-start link and navigates on pair', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();

  expect(mockAlert).toHaveBeenCalledWith(
    '配对到「Studio Mac mini」？',
    expect.stringContaining('100.64.0.7'),
    expect.any(Array),
  );
  await act(async () => alertButtons()[1]?.onPress?.());
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('PairWaiting', {
    qrText: pairQrStudio,
  });
});

it('does not navigate when the user cancels', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  await act(async () => alertButtons()[0]?.onPress?.());
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});

it('replaces PairAdd when the user confirms from the scan screen', async () => {
  mockNavigationRef.getCurrentRoute.mockReturnValue({ name: 'PairAdd' });
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  await act(async () => alertButtons()[1]?.onPress?.());
  expect(mockNavigationRef.dispatch).toHaveBeenCalledWith({
    type: 'REPLACE',
    payload: { name: 'PairWaiting', params: { qrText: pairQrStudio } },
  });
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});

it('waits for onboarding and then processes the pending link', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  const rendered = await renderWithProviders(<DeepLinkPairing />);
  await settle();
  expect(mockAlert).not.toHaveBeenCalled();

  await act(async () => {
    rendered.settingsStore.set('onboarding.done', true);
  });
  await settle();
  expect(mockAlert).toHaveBeenCalledTimes(1);
});

it('waits while locked and resumes after unlock', async () => {
  useLockState.setState({ locked: true });
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(mockAlert).not.toHaveBeenCalled();

  await act(async () => {
    useLockState.getState().setLocked(false);
  });
  await settle();
  expect(mockAlert).toHaveBeenCalledTimes(1);
});

it('deduplicates an initial URL followed by the same event', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  await act(async () => {
    mockUrlListener?.({ url: pairQrStudio });
  });
  await settle();
  expect(mockAlert).toHaveBeenCalledTimes(1);
});

it('waits until navigation is ready', async () => {
  mockNavigationRef.isReady.mockReturnValue(false);
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(mockAlert).not.toHaveBeenCalled();
  mockNavigationRef.isReady.mockReturnValue(true);
  await act(async () => mockNavigationListeners.get('ready')?.());
  expect(mockAlert).toHaveBeenCalledTimes(1);
});

it('rechecks a suspended link when the navigation state changes', async () => {
  mockNavigationRef.getCurrentRoute.mockReturnValue({ name: 'Terminal' });
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(mockAlert).not.toHaveBeenCalled();
  mockNavigationRef.getCurrentRoute.mockReturnValue({ name: 'MainTabs' });
  await act(async () => mockNavigationListeners.get('state')?.());
  expect(mockAlert).toHaveBeenCalledTimes(1);
});

it('keeps only the latest link while locked', async () => {
  useLockState.setState({ locked: true });
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  const latestUrl = pairQrStudio.replace('Studio%20Mac%20mini', 'Second%20Mac');
  await act(async () => mockUrlListener?.({ url: latestUrl }));
  await act(async () => useLockState.getState().setLocked(false));
  expect(mockAlert).toHaveBeenCalledTimes(1);
  expect(mockAlert.mock.calls[0]?.[0]).toBe('配对到「Second Mac」？');
  await act(async () => alertButtons()[1]?.onPress?.());
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('PairWaiting', {
    qrText: latestUrl,
  });
});

it('does not let a late initial URL supersede a runtime event', async () => {
  let resolveInitial: (url: string) => void = () => {};
  mockGetInitialURL.mockReturnValue(
    new Promise(resolve => {
      resolveInitial = resolve;
    }),
  );
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  const latestUrl = pairQrStudio.replace('Studio%20Mac%20mini', 'Second%20Mac');
  await act(async () => mockUrlListener?.({ url: latestUrl }));
  await act(async () => resolveInitial(pairQrStudio));
  expect(mockAlert).toHaveBeenCalledTimes(1);
  expect(mockAlert.mock.calls[0]?.[0]).toBe('配对到「Second Mac」？');
  await act(async () => alertButtons()[0]?.onPress?.());
  expect(mockAlert).toHaveBeenCalledTimes(1);
});

it('queues another link until the current confirmation closes', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  const secondUrl = pairQrStudio.replace('Studio%20Mac%20mini', 'Second%20Mac');
  await act(async () => mockUrlListener?.({ url: secondUrl }));
  expect(mockAlert).toHaveBeenCalledTimes(1);
  await act(async () => alertButtons()[0]?.onPress?.());
  expect(mockAlert).toHaveBeenCalledTimes(2);
  expect(mockAlert.mock.calls[1]?.[0]).toBe('配对到「Second Mac」？');
});

it('deduplicates each URL even when another link arrives between duplicates', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  await act(async () => alertButtons()[0]?.onPress?.());
  const secondUrl = pairQrStudio.replace('Studio%20Mac%20mini', 'Second%20Mac');
  await act(async () => mockUrlListener?.({ url: secondUrl }));
  const buttons = mockAlert.mock.calls[1]?.[2];
  await act(async () => buttons?.[0]?.onPress?.());
  await act(async () => mockUrlListener?.({ url: pairQrStudio }));
  expect(mockAlert).toHaveBeenCalledTimes(2);
});

it('can process the same URL again after ten seconds', async () => {
  jest.useFakeTimers();
  jest.setSystemTime(1_000);
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  await act(async () => alertButtons()[0]?.onPress?.());
  jest.setSystemTime(11_001);
  await act(async () => mockUrlListener?.({ url: pairQrStudio }));
  expect(mockAlert).toHaveBeenCalledTimes(2);
});

it('defers a confirmed link if the App locks while the alert is open', async () => {
  mockGetInitialURL.mockResolvedValue(pairQrStudio);
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  await act(async () => useLockState.getState().setLocked(true));
  await act(async () => alertButtons()[1]?.onPress?.());
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
  await act(async () => useLockState.getState().setLocked(false));
  expect(mockAlert).toHaveBeenCalledTimes(1);
  expect(mockNavigationRef.navigate).toHaveBeenCalledWith('PairWaiting', {
    qrText: pairQrStudio,
  });
});

it('shows a toast for an invalid pair link without opening confirmation', async () => {
  mockGetInitialURL.mockResolvedValue('rsp://pair?v=2');
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(mockAlert).not.toHaveBeenCalled();
  expect(screen.getByText('这不是 Remote Screen 的配对码')).toBeOnTheScreen();
});

it('ignores non-pair links', async () => {
  mockGetInitialURL.mockResolvedValue('rsp://other?v=1');
  await renderWithProviders(<DeepLinkPairing />, {
    settings: { 'onboarding.done': true },
  });
  await settle();
  expect(mockAlert).not.toHaveBeenCalled();
  expect(mockNavigationRef.navigate).not.toHaveBeenCalled();
});
