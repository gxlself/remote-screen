import {
  act,
  cleanup,
  fireEvent,
  renderHook,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { StackActions } from '@react-navigation/native';
import type { QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import type { RootScreenProps } from '../../app/navigation/routes';
import { navigationRef } from '../../app/navigation/navigationRef';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { computers } from '../../native/rscore/fixtures';
import { installFakeAdapters } from '../../features/adapters/testing/fakeAdapters';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionEndedScreen from './index';
import { useEndActions } from './useEndActions';

const HOST_ID = 'h7v2m9q3k8x4tb1a';
const WINDOWS_HOST_ID = 'w3k8p2x7m4q9tb6c';

const queryClients: QueryClient[] = [];

async function renderEndedScreen(
  hostId: string,
  code: number,
  withAdapters: boolean,
) {
  const core = createFakeCore();
  if (withAdapters) {
    installFakeAdapters(core);
  }
  const platform = new FakePlatform();
  const navigation = mockNavigation<'SessionEnded'>();
  const rendered = await renderWithProviders(
    <SessionEndedScreen
      navigation={navigation}
      route={mockRoute('SessionEnded', { hostId, code })}
    />,
    // The paired Hosts as the last reach probe and the tailnet found them.
    { core, platform, localDirectory: { peers: [] } },
  );
  queryClients.push(rendered.queryClient);
  return { ...rendered, navigation, platform };
}

async function renderActions(code: number, withAdapters: boolean) {
  const core = createFakeCore();
  if (withAdapters) {
    installFakeAdapters(core);
  }
  const platform = new FakePlatform();
  const providers = await renderWithProviders(<></>, {
    core,
    platform,
    localDirectory: { peers: [] },
  });
  const navigation = mockNavigation<'SessionEnded'>();
  const host = computers.find(computer => computer.hostId === HOST_ID);
  function Wrapper({ children }: { children: ReactNode }) {
    return providers.wrapper({ children });
  }
  const hook = await renderHook(
    () =>
      useEndActions({
        route: mockRoute('SessionEnded', {
          hostId: HOST_ID,
          code,
        }) as RootScreenProps<'SessionEnded'>['route'],
        navigation,
        host,
      }),
    { wrapper: Wrapper },
  );
  return { ...hook, navigation, platform, queryClient: providers.queryClient };
}

afterEach(async () => {
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
});

/** The end page hands over to the session with replace (V06). */
function spyNavigation() {
  const ready = jest.spyOn(navigationRef, 'isReady').mockReturnValue(true);
  const navigate = jest
    .spyOn(navigationRef, 'navigate')
    .mockImplementation(() => undefined);
  const dispatch = jest
    .spyOn(navigationRef, 'dispatch')
    .mockImplementation(() => undefined);
  return { ready, navigate, dispatch };
}

function expectAdapterNavigation(
  { navigate, dispatch }: ReturnType<typeof spyNavigation>,
  context: 'loginWindow' | 'noHost' | 'fallback',
) {
  expect(dispatch).toHaveBeenCalledWith(
    StackActions.replace('AdapterSession', {
      target: { protocol: 'vnc', hostId: HOST_ID, addr: '100.64.0.7' },
      name: 'Studio Mac mini',
      addrKind: 'tailnet',
      context,
    }),
  );
  expect(navigate).not.toHaveBeenCalled();
}

it('passes the login-window context from board 190', async () => {
  const spies = spyNavigation();
  const rendered = await renderActions(190, true);

  await act(async () => rendered.result.current.openScreenSharing());

  expect(spies.ready).toHaveBeenCalled();
  expectAdapterNavigation(spies, 'loginWindow');
});

it('passes the no-host context from board 120', async () => {
  const spies = spyNavigation();
  const rendered = await renderActions(120, true);

  await act(async () => rendered.result.current.openScreenSharing());

  expectAdapterNavigation(spies, 'noHost');
});

it('passes the fallback context for other boards', async () => {
  const spies = spyNavigation();
  const rendered = await renderActions(161, true);

  await act(async () => rendered.result.current.openScreenSharing());

  expectAdapterNavigation(spies, 'fallback');
});

it('keeps the external-app behavior when adapters are missing', async () => {
  const { navigate, dispatch } = spyNavigation();
  const rendered = await renderActions(161, false);

  await act(async () => rendered.result.current.openScreenSharing());

  expect(rendered.platform.opened).toEqual(['vnc://100.64.0.7']);
  expect(navigate).not.toHaveBeenCalled();
  expect(dispatch).not.toHaveBeenCalled();
});

it('windows_fallback_builtin opens remote desktop from board 101 in the built-in adapter', async () => {
  const { navigate, dispatch } = spyNavigation();
  const office = computers.find(
    computer => computer.hostId === WINDOWS_HOST_ID,
  );
  const rendered = await renderEndedScreen(WINDOWS_HOST_ID, 101, true);

  await fireEvent.press(
    await screen.findByText(sessionEnded.useDesktop('windows')),
  );

  expect(navigate).not.toHaveBeenCalled();
  expect(dispatch).toHaveBeenCalledTimes(1);
  expect(dispatch).toHaveBeenCalledWith(
    StackActions.replace('AdapterSession', {
      target: {
        protocol: 'rdp',
        hostId: WINDOWS_HOST_ID,
        addr: '100.64.0.12',
      },
      name: office?.name,
      addrKind: 'tailnet',
      context: 'fallback',
    }),
  );
  // No port means the RDP default 3389 (C6.1); the Host's 7400 must not leak.
  const [action] = dispatch.mock.calls[0] as unknown as [
    { payload: { params: { target: object } } },
  ];
  expect(action.payload.params.target).not.toHaveProperty('port');
  expect(rendered.platform.calls).toEqual([]);
});

it('keeps remote desktop on the external app when adapters are unavailable', async () => {
  const { navigate, dispatch } = spyNavigation();
  const rendered = await renderEndedScreen(WINDOWS_HOST_ID, 101, false);

  await fireEvent.press(
    await screen.findByText(sessionEnded.useDesktop('windows')),
  );

  await waitFor(() => {
    expect(rendered.platform.opened).toEqual([
      'rdp://full%20address=s:100.64.0.12:3389',
    ]);
  });
  expect(navigate).not.toHaveBeenCalled();
  expect(dispatch).not.toHaveBeenCalled();
});
