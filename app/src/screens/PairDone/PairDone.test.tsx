import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { directory } from '../../native/rscore/fixtures';
import type { Address, Directory } from '../../native/rscore/types';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import PairDoneScreen from '.';

const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';

// Same Wi-Fi needs an address's subnet, which only a Hub reported; the
// directory is handed to the screen as it was.
let mockDirectory: Directory = directory;
jest.mock('../../features/data/directoryQuery', () => ({
  ...jest.requireActual('../../features/data/directoryQuery'),
  useDirectory: () => ({
    directory: mockDirectory,
    isLoading: false,
    error: null,
    refetch: () => Promise.resolve(),
  }),
}));

beforeEach(() => {
  mockDirectory = directory;
});

type Params = RootScreenProps<'PairDone'>['route']['params'];

function props(params: Params) {
  return {
    navigation: mockNavigation<'PairDone'>(),
    route: mockRoute('PairDone', params),
  } as RootScreenProps<'PairDone'>;
}

async function renderPairDone(
  params?: Partial<Params>,
  core = createFakeCore({ directory }),
  tailnetEnabled = true,
) {
  const screenProps = props({
    hostId: STUDIO_HOST_ID,
    hostName: 'Studio Mac mini',
    permission: 'unattended',
    path: 'tailnetDirect',
    latencyMs: 6,
    ...params,
  });
  const view = await renderWithProviders(<PairDoneScreen {...screenProps} />, {
    core,
    settings: { 'tailnet.enabled': tailnetEnabled },
  });
  await act(async () => {
    await Promise.resolve();
    await new Promise<void>(resolve => {
      setTimeout(resolve, 0);
    });
  });
  return { ...view, ...screenProps };
}

afterEach(async () => {
  await cleanup();
});

describe('PairDone', () => {
  it('shows the paired host, permission, addresses and current path', async () => {
    await renderPairDone();
    expect(screen.getByText('Studio Mac mini 已配对')).toBeOnTheScreen();
    expect(screen.getByText('允许控制')).toBeOnTheScreen();
    expect(screen.getByText('无人值守')).toBeOnTheScreen();
    expect(screen.getByText('学到的连接地址')).toBeOnTheScreen();
    expect(await screen.findByText('2 个')).toBeOnTheScreen();
    expect(screen.getByText('100.64.0.7')).toBeOnTheScreen();
    expect(screen.getByText('在哪都能连 · 刚才就是走这条')).toBeOnTheScreen();
    expect(screen.getByText('192.168.31.20')).toBeOnTheScreen();
    expect(screen.getByText('同一 Wi-Fi')).toBeOnTheScreen();
  });

  it('leaves the tailnet address out while Tailscale is switched off', async () => {
    await renderPairDone({ path: 'lan' }, undefined, false);
    expect(await screen.findByText('1 个')).toBeOnTheScreen();
    expect(screen.getByText('192.168.31.20')).toBeOnTheScreen();
    expect(screen.queryByText('100.64.0.7')).toBeNull();
    expect(screen.queryByText(/在哪都能连/)).toBeNull();
  });

  it('names a tailnet path it just used neutrally while Tailscale is off', async () => {
    await renderPairDone(undefined, undefined, false);
    expect(await screen.findByText('2 个')).toBeOnTheScreen();
    expect(
      screen.getByText('电脑的另一个地址 · 刚才就是走这条'),
    ).toBeOnTheScreen();
    expect(screen.queryByText(/在哪都能连/)).toBeNull();
  });

  it('seeds lock.sensitiveHosts on open and toggles it off', async () => {
    const { settingsStore } = await renderPairDone();
    expect(settingsStore.get('lock.sensitiveHosts')).toContain(STUDIO_HOST_ID);
    await fireEvent.press(screen.getByLabelText('连接前验证 Face ID'));
    expect(settingsStore.get('lock.sensitiveHosts')).not.toContain(
      STUDIO_HOST_ID,
    );
  });

  it('names Touch ID in the sensitive host switch', async () => {
    const core = createFakeCore({ directory });
    const platform = createFakePlatform();
    platform.authCapabilityResult = { biometry: 'touchId', passcode: true };
    await renderWithProviders(
      <PairDoneScreen
        {...props({
          hostId: STUDIO_HOST_ID,
          hostName: 'Studio Mac mini',
          permission: 'unattended',
          path: 'tailnetDirect',
          latencyMs: 6,
        })}
      />,
      { core, platform },
    );

    expect(screen.getByLabelText('连接前验证 Touch ID')).toBeOnTheScreen();
  });

  it('connects now and returns to devices', async () => {
    const { navigation } = await renderPairDone();
    await fireEvent.press(screen.getByRole('button', { name: '现在连接' }));
    expect(navigation.reset).toHaveBeenCalledWith({
      index: 1,
      routes: [
        { name: 'MainTabs' },
        { name: 'Session', params: { hostId: STUDIO_HOST_ID } },
      ],
    });
    await fireEvent.press(screen.getByRole('button', { name: '回到设备' }));
    expect(navigation.reset).toHaveBeenCalledWith({
      index: 0,
      routes: [{ name: 'MainTabs' }],
    });
  });

  it('treats back or a swipe as 回到设备 instead of the old pairing page', async () => {
    const { navigation } = await renderPairDone();
    const addListener = navigation.addListener as unknown as jest.Mock;
    const calls = addListener.mock.calls.filter(
      ([name]) => name === 'beforeRemove',
    );
    const beforeRemove = calls[calls.length - 1][1] as (event: {
      preventDefault: () => void;
      data: { action: { type: string } };
    }) => void;

    const back = { preventDefault: jest.fn(), data: { action: { type: 'GO_BACK' } } };
    await act(async () => beforeRemove(back));
    expect(back.preventDefault).toHaveBeenCalledTimes(1);
    expect(navigation.reset).toHaveBeenCalledWith({
      index: 0,
      routes: [{ name: 'MainTabs' }],
    });

    // The reset itself must go through.
    const reset = { preventDefault: jest.fn(), data: { action: { type: 'RESET' } } };
    await act(async () => beforeRemove(reset));
    expect(reset.preventDefault).not.toHaveBeenCalled();
  });

  it('explains the two paths only when there are two addresses', async () => {
    mockDirectory = {
      ...directory,
      computers: directory.computers.map(computer =>
        computer.hostId === STUDIO_HOST_ID
          ? {
              ...computer,
              addresses: [{ kind: 'tailnet', addr: '100.64.0.7', port: 7400 }],
            }
          : computer,
      ),
    };
    await renderPairDone();
    expect(screen.getByText('100.64.0.7')).toBeOnTheScreen();
    expect(
      screen.queryByText('两条路都认这台手机的设备密钥，换网络不用重新配对。'),
    ).toBeNull();
  });

  it('calls a Windows host 电脑 and names the tray', async () => {
    mockDirectory = {
      ...directory,
      computers: directory.computers.map(computer =>
        computer.hostId === STUDIO_HOST_ID
          ? { ...computer, os: 'windows', osVersion: 'Windows 11' }
          : computer,
      ),
    };
    await renderPairDone();
    expect(screen.getByText('电脑给了这台手机')).toBeOnTheScreen();
    expect(screen.getByText(/托盘的 Host 里关掉就行/)).toBeOnTheScreen();
    expect(screen.queryByText('Mac 给了这台手机')).toBeNull();
  });

  it('omits the permission section when permission is absent', async () => {
    await renderPairDone({ permission: undefined });
    expect(screen.queryByText('Mac 给了这台手机')).toBeNull();
    expect(screen.queryByText('允许控制')).toBeNull();
  });

  it('matches a Wi-Fi interface address that carries a prefix', async () => {
    const addresses: Address[] = [
      { kind: 'tailnet', addr: '100.64.0.7', port: 7400 },
      { kind: 'lan', addr: '192.168.1.20', port: 7400, cidr: '192.168.1.0/24' },
    ];
    const homeLan: Directory = {
      ...directory,
      computers: directory.computers.map(computer =>
        computer.hostId === STUDIO_HOST_ID
          ? { ...computer, addresses }
          : computer,
      ),
    };
    mockDirectory = homeLan;
    const core = createFakeCore({
      network: {
        ifaces: [{ name: 'en0', addrs: ['192.168.1.5/24'], isVpn: false }],
      },
    });
    await renderPairDone(undefined, core);
    expect(await screen.findByText('192.168.1.20')).toBeOnTheScreen();
    expect(await screen.findByText('同一 Wi-Fi')).toBeOnTheScreen();
  });
});
