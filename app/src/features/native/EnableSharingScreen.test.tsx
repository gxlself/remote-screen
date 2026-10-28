import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { RootStackParamList } from '../../app/navigation/routes';
import { native } from '../../i18n/zh/native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { computers } from '../../native/rscore/fixtures';
import type { DiscoverProbe } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { installFakeAdapters } from '../adapters/testing/fakeAdapters';
import type { FallbackGuideState } from '../fallback/useRemoteFallback';
import EnableSharingScreen from './EnableSharingScreen';
import type { ComputerEntry } from './types';
import type { ConnectOptions } from './useConnect';

const mockConnect = jest.fn<Promise<void>, [ComputerEntry, ConnectOptions?]>();
const mockCloseGuide = jest.fn();
let mockBusy = false;
let mockGuide: FallbackGuideState = null;
let mockRealConnect = false;
let mockEntries: ComputerEntry[] = [];

jest.mock('./useConnect', () => {
  const actual =
    jest.requireActual<typeof import('./useConnect')>('./useConnect');
  return {
    // A test keeps one choice for its whole render, so the hook order is stable.
    useConnect: () =>
      mockRealConnect
        ? actual.useConnect()
        : {
            connect: mockConnect,
            busy: mockBusy,
            guide: mockGuide,
            closeGuide: mockCloseGuide,
          },
  };
});
jest.mock('./useComputerEntries', () => ({
  useComputerEntries: () => mockEntries,
}));
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    navigate: jest.fn(),
    dispatch: jest.fn(),
    isReady: jest.fn(() => true),
  },
}));

const navigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as { navigate: jest.Mock; dispatch: jest.Mock };

const studio = computers[0];
const hostEntry: ComputerEntry = {
  kind: 'host',
  key: `host:${studio.hostId}`,
  computer: studio,
};
const tailnetEntry: ComputerEntry = {
  kind: 'tailnet',
  key: 'tailnet:office',
  device: {
    node: 'office',
    name: 'Office PC',
    os: 'windows',
    addrs: ['100.64.0.40'],
    online: true,
    lastSeenAt: null,
  },
};

const clients: QueryClient[] = [];

async function renderScreen(
  params: RootStackParamList['EnableSharing'],
  core: FakeCore = createFakeCore(),
) {
  const navigation = mockNavigation<'EnableSharing'>();
  const view = await renderWithProviders(
    <EnableSharingScreen
      navigation={navigation}
      route={mockRoute('EnableSharing', params)}
    />,
    { core },
  );
  clients.push(view.queryClient);
  return { ...view, navigation, core };
}

function stepTexts(): string[] {
  return within(screen.getByTestId('enable-sharing-steps'))
    .queryAllByText(/./)
    .map(node => String(node.props.children));
}

beforeEach(() => {
  mockConnect.mockReset();
  mockConnect.mockResolvedValue(undefined);
  mockBusy = false;
  mockGuide = null;
  mockCloseGuide.mockReset();
  mockRealConnect = false;
  mockEntries = [hostEntry, tailnetEntry];
  navigationRef.navigate.mockReset();
  navigationRef.dispatch.mockReset();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
});

it('shows the numbered Mac steps, which switch a setting and install nothing', async () => {
  await renderScreen({
    os: 'macos',
    name: studio.name,
    entryKey: hostEntry.key,
  });

  expect(
    screen.getByRole('header', { name: native.enable.title('macos') }),
  ).toBeOnTheScreen();
  expect(
    screen.getByText(native.enable.lead('macos', studio.name)),
  ).toBeOnTheScreen();
  const texts = stepTexts();
  for (const [index, step] of native.enable.stepsMac.entries()) {
    expect(texts).toContain(String(index + 1));
    expect(texts).toContain(step);
  }
  for (const step of native.enable.stepsWin) {
    expect(texts).not.toContain(step);
  }
  expect(texts.join('\n')).not.toMatch(/下载|安装/);
});

it('shows the Windows steps without a name', async () => {
  await renderScreen({ os: 'windows' });

  expect(
    screen.getByRole('header', { name: native.enable.title('windows') }),
  ).toBeOnTheScreen();
  expect(screen.getByText(native.enable.lead('windows'))).toBeOnTheScreen();
  const texts = stepTexts();
  for (const step of native.enable.stepsWin) {
    expect(texts).toContain(step);
  }
  for (const step of native.enable.stepsMac) {
    expect(texts).not.toContain(step);
  }
  expect(texts.join('\n')).not.toMatch(/家庭版/);
});

it('retries C7 with the entry found by entryKey', async () => {
  const { navigation } = await renderScreen({
    os: 'windows',
    name: 'Office PC',
    entryKey: tailnetEntry.key,
  });

  await fireEvent.press(screen.getByTestId('enable-sharing-retry'));

  expect(mockConnect).toHaveBeenCalledTimes(1);
  // The session replaces this guide; a still-closed port stays on this page.
  expect(mockConnect).toHaveBeenCalledWith(
    tailnetEntry,
    expect.objectContaining({ replace: true, onSharingOff: expect.any(Function) }),
  );
  expect(navigation.goBack).not.toHaveBeenCalled();
});

it('says sharing is still off in red instead of opening the guide again', async () => {
  mockConnect.mockImplementation(async (_entry, options) => {
    options?.onSharingOff?.('macos');
  });
  const { navigation } = await renderScreen({
    os: 'macos',
    name: studio.name,
    entryKey: hostEntry.key,
  });
  expect(screen.queryByTestId('enable-sharing-still-off')).toBeNull();

  await fireEvent.press(screen.getByTestId('enable-sharing-retry'));

  expect(screen.getByTestId('enable-sharing-still-off')).toHaveTextContent(
    native.enable.stillOff('macos'),
  );
  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

it('goes back when the entry is no longer in the list', async () => {
  const gone = await renderScreen({
    os: 'macos',
    name: 'Old Mac',
    entryKey: 'tailnet:gone',
  });
  await fireEvent.press(screen.getByTestId('enable-sharing-retry'));
  expect(gone.navigation.goBack).toHaveBeenCalledTimes(1);
  await gone.unmount();

  const keyless = await renderScreen({ os: 'macos', name: 'Mac' });
  await fireEvent.press(screen.getByTestId('enable-sharing-retry'));
  expect(keyless.navigation.goBack).toHaveBeenCalledTimes(1);

  expect(mockConnect).not.toHaveBeenCalled();
});

it('cannot retry again while a connection is in progress', async () => {
  mockBusy = true;
  await renderScreen({ os: 'macos', entryKey: hostEntry.key });

  const retry = screen.getByTestId('enable-sharing-retry');
  expect(retry).toBeDisabled();
  expect(retry).toBeBusy();
  await fireEvent.press(retry);
  expect(mockConnect).not.toHaveBeenCalled();
});

it('ignores a second tap while the real C7 probe is still running', async () => {
  mockRealConnect = true;
  mockEntries = [hostEntry];
  const core = createFakeCore();
  installFakeAdapters(core);
  let finishProbe: (result: DiscoverProbe) => void = () => undefined;
  const probe = jest.spyOn(core.discover!, 'probe').mockImplementation(
    () =>
      new Promise<DiscoverProbe>(resolve => {
        finishProbe = resolve;
      }),
  );
  await renderScreen(
    { os: 'macos', name: studio.name, entryKey: hostEntry.key },
    core,
  );

  await fireEvent.press(screen.getByTestId('enable-sharing-retry'));
  expect(screen.getByTestId('enable-sharing-retry')).toBeDisabled();
  await fireEvent.press(screen.getByTestId('enable-sharing-retry'));
  expect(probe).toHaveBeenCalledTimes(1);

  await act(async () => {
    finishProbe({ addr: '100.64.0.7', port: 5900, outcome: 'open', ms: 4 });
  });

  expect(navigationRef.dispatch).toHaveBeenCalledTimes(1);
  expect(navigationRef.dispatch).toHaveBeenCalledWith({
    type: 'REPLACE',
    payload: {
      name: 'AdapterSession',
      params: expect.objectContaining({
        target: expect.objectContaining({
          protocol: 'vnc',
          hostId: studio.hostId,
        }),
        context: 'native',
      }),
    },
  });
  expect(navigationRef.navigate).not.toHaveBeenCalled();
  expect(screen.getByTestId('enable-sharing-retry')).toBeEnabled();
});

it('shows the external-app guide that the retry raised', async () => {
  mockGuide = { kind: 'rdp' };
  await renderScreen({ os: 'windows', entryKey: tailnetEntry.key });

  const guide = screen.getByTestId('fallback-guide-card');
  expect(within(guide).getByText('需要 Windows App')).toBeOnTheScreen();
  await fireEvent.press(within(guide).getByRole('button', { name: '取消' }));
  expect(mockCloseGuide).toHaveBeenCalledTimes(1);
});

it('offers installing the Host as the last step', async () => {
  const { navigation } = await renderScreen({ os: 'macos' });

  const labels = screen
    .getAllByRole('button')
    .map(button => button.props.accessibilityLabel as string);
  expect(labels.at(-1)).toBe(native.enable.installHost);
  await fireEvent.press(screen.getByText(native.enable.installHost));
  // Goes back to an open PairAdd instead of stacking a second one.
  expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', undefined, {
    pop: true,
  });
  expect(mockConnect).not.toHaveBeenCalled();
});

it('goes back from the header button', async () => {
  const { navigation } = await renderScreen({ os: 'windows' });

  await fireEvent.press(screen.getByLabelText('返回'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});
