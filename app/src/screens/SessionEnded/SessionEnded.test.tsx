import {
  act,
  cleanup,
  fireEvent,
  renderHook,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { ReactNode } from 'react';
import type { QueryClient } from '@tanstack/react-query';
import type { RootScreenProps } from '../../app/navigation/routes';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { computers } from '../../native/rscore/fixtures';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionEndedScreen from './index';
import { useEndActions } from './useEndActions';

const STUDIO = 'h7v2m9q3k8x4tb1a';
const OFFICE = 'w3k8p2x7m4q9tb6c';

const queryClients: QueryClient[] = [];

type ScreenOptions = {
  platform?: FakePlatform;
};

async function renderScreen(
  params: RootScreenProps<'SessionEnded'>['route']['params'],
  opts: ScreenOptions = {},
) {
  const navigation = mockNavigation<'SessionEnded'>();
  const core = createFakeCore();
  const platform = opts.platform ?? new FakePlatform();
  const rendered = await renderWithProviders(
    <SessionEndedScreen
      navigation={navigation as never}
      route={mockRoute('SessionEnded', params) as never}
    />,
    {
      core,
      platform,
      // The paired Hosts as the last reach probe and the tailnet found them.
      localDirectory: { peers: [] },
    },
  );
  queryClients.push(rendered.queryClient);
  // Let the directory query settle so the host name update lands in act.
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 0));
    });
  }
  return { ...rendered, navigation };
}

async function renderActions(hostId: string, platform: FakePlatform) {
  const navigation = mockNavigation<'SessionEnded'>();
  const providers = await renderWithProviders(<></>, {
    platform,
    localDirectory: { peers: [] },
  });
  queryClients.push(providers.queryClient);
  const host = computers.find(computer => computer.hostId === hostId);
  function Wrapper({ children }: { children: ReactNode }) {
    return providers.wrapper({ children });
  }
  const hook = await renderHook(
    () =>
      useEndActions({
        route: mockRoute('SessionEnded', { hostId }),
        navigation: navigation as never,
        host,
      }),
    { wrapper: Wrapper },
  );
  return { ...hook, navigation, platform };
}

afterEach(async () => {
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
});

describe('SessionEnded', () => {
  it('renders E142 and wires retry, back and context logging', async () => {
    const { navigation, core } = await renderScreen({
      hostId: STUDIO,
      code: 142,
    });
    await waitFor(() => {
      expect(screen.getByText('Studio Mac mini 的会话已满')).toBeOnTheScreen();
    });

    await fireEvent.press(screen.getByText('重试'));
    expect(navigation.replace).toHaveBeenCalledWith('Session', {
      hostId: STUDIO,
    });

    await fireEvent.press(screen.getByText('返回'));
    expect(navigation.goBack).toHaveBeenCalledTimes(1);

    const log = core.calls.find(call => call.method === 'logs.write');
    expect(log).toBeDefined();
    expect(log?.args[0]).toBe('warn');
    expect(log?.args[1]).toBe('session-ended');
    expect(log?.args[2]).toContain('"code":142');
  });

  it('retries as a viewer when requested', async () => {
    const platform = new FakePlatform();
    const { result, navigation } = await renderActions(STUDIO, platform);
    await act(async () => result.current.retry({ viewOnly: true }));
    expect(navigation.replace).toHaveBeenCalledWith('Session', {
      hostId: STUDIO,
      viewOnly: true,
    });
  });

  it('passes an explicit false through when retrying', async () => {
    const platform = new FakePlatform();
    const { result, navigation } = await renderActions(STUDIO, platform);
    await act(async () => result.current.retry({ viewOnly: false }));
    expect(navigation.replace).toHaveBeenCalledWith('Session', {
      hostId: STUDIO,
      viewOnly: false,
    });
  });

  it('starts pairing again for E180', async () => {
    const { navigation } = await renderScreen({ hostId: STUDIO, code: 180 });
    await fireEvent.press(screen.getByText('重新配对'));
    expect(navigation.replace).toHaveBeenCalledWith('PairAdd', {
      tab: 'scan',
      hostId: STUDIO,
    });
  });

  it('shows an idle timeout without a code button', async () => {
    await renderScreen({ hostId: STUDIO, reason: 'idleTimeout' });
    expect(screen.getByText('空闲太久，连接已断开')).toBeOnTheScreen();
    expect(screen.queryByLabelText('复制错误码 RS-E101')).toBeNull();
    expect(screen.queryByText('RS-E101')).toBeNull();
    expect(screen.queryByLabelText(/复制错误码/)).toBeNull();
  });

  it('copies E101 from the permission placeholder and shows a toast', async () => {
    const platform = new FakePlatform();
    await renderScreen({ hostId: STUDIO, code: 101 }, { platform });
    await fireEvent.press(screen.getByLabelText('复制错误码 RS-E101'));
    expect(platform.copied).toContain('RS-E101');
    expect(screen.getByText('已复制错误码')).toBeOnTheScreen();
  });

  it('opens diagnostics for E199', async () => {
    const { navigation } = await renderScreen({ hostId: STUDIO, code: 199 });
    await fireEvent.press(screen.getByText('诊断'));
    expect(navigation.navigate).toHaveBeenCalledWith('Diagnostics', {
      hostId: STUDIO,
    });
  });

  it('opens the Mac tailnet screen-sharing URL', async () => {
    const platform = new FakePlatform();
    const { result } = await renderActions(STUDIO, platform);
    await act(async () => {
      await result.current.openScreenSharing();
    });
    expect(platform.opened).toEqual(['vnc://100.64.0.7']);
  });

  it('opens the Windows RDP URL', async () => {
    const platform = new FakePlatform();
    const { result } = await renderActions(OFFICE, platform);
    await act(async () => {
      await result.current.openScreenSharing();
    });
    expect(platform.opened).toEqual([
      'rdp://full%20address=s:100.64.0.12:3389',
    ]);
  });

  it('shows a guide when no screen-sharing app can open the URL', async () => {
    const platform = new FakePlatform();
    platform.openUrlResult = false;
    const { result } = await renderActions(STUDIO, platform);
    await act(async () => {
      await result.current.openScreenSharing();
    });
    expect(result.current.guide).toEqual({ kind: 'vnc' });
    expect(result.current.toastState).toBeNull();
  });

  it('falls back to a plain name, not the host id (V60)', async () => {
    await renderScreen({ hostId: 'z9z9z9z9z9z9z9z9', code: 142 });
    expect(screen.getByText('这台电脑 的会话已满')).toBeOnTheScreen();
    expect(screen.queryByText(/z9z9z9z9/)).toBeNull();
  });

  it('keeps the route name over the directory', async () => {
    await renderScreen({
      hostId: 'z9z9z9z9z9z9z9z9',
      hostName: '书房 Mac',
      code: 142,
    });
    expect(screen.getByText('书房 Mac 的会话已满')).toBeOnTheScreen();
  });

  it('does not assume macOS for an unknown computer (V60)', async () => {
    await renderScreen({ hostId: 'z9z9z9z9z9z9z9z9', code: 120 });
    expect(screen.getByText('用屏幕共享 / 远程桌面连接')).toBeOnTheScreen();
    expect(screen.queryByText(/Mac/)).toBeNull();
  });

  it('does not invent a code when one is absent', async () => {
    const platform = new FakePlatform();
    const { result } = await renderActions(STUDIO, platform);
    await act(async () => result.current.copyCode());
    expect(platform.copied).toEqual(['RS-E199']);
  });
});
