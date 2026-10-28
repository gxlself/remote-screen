import { act, cleanup, screen } from '@testing-library/react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW, pairedHosts } from '../../native/rscore/fixtures';
import type { TailscaleDevice } from '../../native/rscore/types';
import HomeScreen from '../../screens/Home';
import { mockPhoneDimensions } from '../../test/dimensions';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';

jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: { isReady: () => true, navigate: jest.fn() },
}));

const OFFICE_ID = 'w3k8p2x7m4q9tb6c';

let restoreDimensions: (() => void) | undefined;

function device(overrides: Partial<TailscaleDevice>): TailscaleDevice {
  return {
    id: 'n1',
    name: 'node',
    dnsName: 'node.tail1234.ts.net',
    os: 'linux',
    addresses: [],
    online: true,
    lastSeenAt: null,
    tags: [],
    ...overrides,
  };
}

async function renderHome(core: ReturnType<typeof createFakeCore>) {
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation'];
  const rendered = await renderWithProviders(
    <HomeScreen
      navigation={navigation}
      route={{ key: 'Home-test', name: 'Home' }}
    />,
    // Tailscale API devices are only listed with 外网访问 on.
    { core, settings: { 'tailnet.enabled': true } },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return rendered;
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockPhoneDimensions();
});

afterEach(async () => {
  await cleanup();
  restoreDimensions?.();
  restoreDimensions = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('lists the Mac and the Windows PC from a saved Tailscale token without a Hub', async () => {
  const core = createFakeCore({
    pairedHosts: pairedHosts.filter(host => host.hostId === OFFICE_ID),
    sshServers: [],
    sessions: [],
  });
  core.setTailscale({
    configured: true,
    devices: [
      device({ id: 'n1', name: 'aliyun-bj', addresses: ['100.64.0.60'] }),
      device({
        id: 'n2',
        name: 'alice-mac',
        os: 'macos',
        addresses: ['100.64.0.61'],
      }),
      device({
        id: 'n3',
        name: 'bob-pc',
        os: 'windows',
        addresses: ['100.64.0.62'],
        online: null,
        lastSeenAt: FIXTURE_NOW - 60_000_000,
      }),
      // Same address as the paired office PC: it stays a Host card.
      device({
        id: 'n4',
        name: 'office-win',
        os: 'windows',
        addresses: ['100.64.0.12'],
      }),
    ],
  });
  await renderHome(core);

  expect(await screen.findByTestId('card-tailnet:alice-mac')).toBeOnTheScreen();
  expect(screen.getByTestId('card-tailnet:bob-pc')).toBeOnTheScreen();
  expect(screen.getByTestId(`card-${OFFICE_ID}`)).toBeOnTheScreen();
  expect(screen.queryByTestId('card-tailnet:office-win')).toBeNull();
  expect(screen.getByText('Mac · 屏幕共享')).toBeOnTheScreen();
  expect(screen.getByText('Windows · 远程桌面')).toBeOnTheScreen();
  expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(false);
});

it('lists Headscale devices by their open ports', async () => {
  const core = createFakeCore({
    pairedHosts: pairedHosts.filter(host => host.hostId === OFFICE_ID),
    sshServers: [],
    sessions: [],
  });
  core.setTailscale({
    configured: true,
    controlUrl: 'https://hs.example.com',
    devices: ['studio', 'gaming-pc', 'gpu-box', 'quiet'].map((name, index) =>
      device({
        id: String(index + 1),
        name,
        dnsName: '',
        os: 'unknown',
        addresses: [`100.64.0.${81 + index}`],
      }),
    ),
  });
  const open = [
    '100.64.0.81:5900',
    '100.64.0.81:22',
    '100.64.0.82:3389',
    '100.64.0.83:22',
  ];
  if (core.discover === undefined) {
    throw new Error('fake discovery missing');
  }
  core.discover.probe = jest.fn(async (addr, port, _kind, ms) => ({
    addr,
    port,
    outcome: open.includes(`${addr}:${port}`)
      ? ('open' as const)
      : ('timeout' as const),
    ms,
  }));
  await renderHome(core);

  expect(await screen.findByTestId('card-tailnet:studio')).toBeOnTheScreen();
  expect(screen.getByTestId('card-tailnet:gaming-pc')).toBeOnTheScreen();
  expect(screen.queryByTestId('card-tailnet:gpu-box')).toBeNull();
  expect(screen.queryByTestId('card-tailnet:quiet')).toBeNull();
  expect(screen.getByText('Mac · 屏幕共享')).toBeOnTheScreen();
  expect(screen.getByText('Windows · 远程桌面')).toBeOnTheScreen();
  expect(screen.getByTestId('server-suggest:studio')).toBeOnTheScreen();
  expect(screen.getByTestId('server-suggest:gpu-box')).toBeOnTheScreen();
  expect(screen.queryByTestId('server-suggest:gaming-pc')).toBeNull();
});
