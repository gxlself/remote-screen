import { AccessibilityInfo, Platform } from 'react-native';
import { act, cleanup, screen } from '@testing-library/react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  netCompanyVpn,
  netNoTailnet,
  netTailnetOk,
} from '../../native/rscore/fixtures';
import { mockNavigation } from '../../test/navigation';
import { mockPhoneDimensions } from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import { home } from '../../i18n/zh/home';
import HomeScreen from '.';

let restoreDimensions: (() => void) | undefined;

async function renderHome(options?: {
  core?: ReturnType<typeof createFakeCore>;
  settings?: {
    networks: {
      id: string;
      name: string;
      cidrs: string[];
      vpnPools: string[];
    }[];
  };
}) {
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation'];
  const rendered = await renderWithProviders(
    <HomeScreen
      navigation={navigation}
      route={{ key: 'Home-announce-test', name: 'Home' }}
    />,
    {
      ...options,
      // These describe Tailscale on Home, which needs 外网访问 on.
      settings: { 'tailnet.enabled': true, ...options?.settings },
      localDirectory: {},
    },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  await act(async () => {
    await Promise.resolve();
  });
  return { ...rendered, navigation };
}

const COMPANY_NETWORK = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

beforeEach(() => {
  jest.clearAllMocks();
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  jest.replaceProperty(Platform, 'OS', 'ios');
  restoreDimensions = mockPhoneDimensions();
});

afterEach(async () => {
  await cleanup();
  restoreDimensions?.();
  restoreDimensions = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('announces the iOS tailnet status once on online to disconnected', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const core = createFakeCore();
  await renderHome({ core });

  expect(announce).not.toHaveBeenCalled();

  await act(async () => {
    core.emit({ type: 'network.changed', status: netNoTailnet });
  });

  expect(announce).toHaveBeenCalledTimes(1);
  expect(announce).toHaveBeenCalledWith(home.status.tailnetDown);
  expect(screen.getByText(home.banners.tailnetDownTitle)).toBeOnTheScreen();
});

it('does not announce a cold start that is already disconnected', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  await renderHome({ core: createFakeCore({ network: netNoTailnet }) });

  expect(announce).not.toHaveBeenCalled();
  expect(screen.getByText(home.banners.tailnetDownTitle)).toBeOnTheScreen();
});

it('announces the company VPN status once on online to disconnected', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const core = createFakeCore();
  await renderHome({ core, settings: { networks: [COMPANY_NETWORK] } });

  await act(async () => {
    core.emit({ type: 'network.changed', status: netCompanyVpn });
  });

  expect(announce).toHaveBeenCalledTimes(1);
  expect(announce).toHaveBeenCalledWith(home.status.tailnetDownCompanyVpn);
  expect(screen.getByTestId('home-banner-vpnConflict')).toBeOnTheScreen();
});

it('announces the online status once on disconnected to online', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const core = createFakeCore({ network: netNoTailnet });
  await renderHome({ core });

  expect(announce).not.toHaveBeenCalled();

  await act(async () => {
    core.emit({ type: 'network.changed', status: netTailnetOk });
  });

  expect(announce).toHaveBeenCalledTimes(1);
  // Studio and the office PC are the paired computers online.
  expect(announce).toHaveBeenCalledWith(home.status.tailnetOnline(2));
});

it('announces only once when tailnet state precedes the network update', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  const core = createFakeCore();
  await renderHome({ core });
  await act(async () => {
    await Promise.resolve();
  });
  const initialCallCount = announce.mock.calls.length;

  await act(async () => {
    core.emit({
      type: 'tailnet.state',
      state: 'unavailable',
      authUrl: null,
      ips: [],
      derpRegion: '',
      message: null,
    });
    core.emit({ type: 'network.changed', status: netNoTailnet });
  });

  await act(async () => {
    await Promise.resolve();
  });

  expect(announce.mock.calls.slice(initialCallCount)).toEqual([
    [home.status.tailnetDown],
  ]);
});
