import { AccessibilityInfo, Platform, StyleSheet } from 'react-native';
import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  netCompanyVpn,
  netCompanyVpnEmbedded,
  netNoTailnet,
} from '../../native/rscore/fixtures';
import type { NetworkStatus } from '../../native/rscore/types';
import { FakePlatform } from '../../native/platform/fakePlatform';
import type { NetworkEntry } from '../../features/settings/schema';
import { resetTailnetSwitchStore } from '../../features/tailnet/switchStore';
import { mockNavigation } from '../../test/navigation';
import { mockPhoneDimensions } from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import HomeScreen from '.';

const COMPANY_NETWORK: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

let restoreDimensions: (() => void) | undefined;

async function renderHome(options?: {
  core?: ReturnType<typeof createFakeCore>;
  platform?: FakePlatform;
  settings?: { networks: NetworkEntry[] };
}) {
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation'];
  const rendered = await renderWithProviders(
    <HomeScreen
      navigation={navigation}
      route={{ key: 'Home-android-test', name: 'Home' }}
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
  return { ...rendered, navigation };
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockPhoneDimensions();
});

afterEach(async () => {
  await act(async () => {
    await Promise.resolve();
  });
  await cleanup();
  jest.clearAllTimers();
  restoreDimensions?.();
  restoreDimensions = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('android', () => {
  beforeEach(() => {
    jest.replaceProperty(Platform, 'OS', 'android');
  });

  it('explains the embedded connection, switches and offers undo', async () => {
    resetTailnetSwitchStore();
    const core = createFakeCore({
      tailnetAvailable: true,
      network: netCompanyVpn,
    });
    const settings = {
      networks: [COMPANY_NETWORK],
      'tailnet.controlUrl': 'https://hs.example.com',
    };
    const { settingsStore } = await renderHome({ core, settings });

    expect(screen.getByText('Android 同时只能开一个 VPN')).toBeOnTheScreen();
    expect(screen.getByText('改用 App 内连接')).toBeOnTheScreen();
    expect(screen.getByText('为什么')).toBeOnTheScreen();
    expect(screen.queryByText('怎么办')).toBeNull();

    await fireEvent.press(screen.getByTestId('home-banner-vpnConflict-why'));
    expect(screen.getByTestId('home-vpn-info')).toHaveTextContent(
      /公司 VPN 占住了系统 VPN/,
    );
    expect(screen.getByTestId('home-vpn-info')).toHaveTextContent(
      /公司 VPN 装在工作资料里？/,
    );
    expect(screen.getByTestId('home-vpn-info')).toHaveTextContent(/已连接/);
    expect(screen.getByTestId('home-vpn-info')).toHaveTextContent(/推荐/);
    await fireEvent.press(screen.getByTestId('home-vpn-info-use-embedded'));
    expect(screen.queryByTestId('home-vpn-info')).toBeNull();
    expect(settingsStore.get('tailnet.mode')).toBe('embedded');

    await act(async () => {
      core.emit({ type: 'network.changed', status: netCompanyVpnEmbedded });
    });
    expect(screen.queryByTestId('home-banner-vpnConflict')).toBeNull();
    expect(screen.getByText('Tailscale 和公司 VPN 同时可用')).toBeOnTheScreen();
    expect(screen.getByText('撤销')).toBeOnTheScreen();
    await fireEvent.press(
      screen.getByTestId('home-banner-embeddedBoth-secondary'),
    );
    expect(settingsStore.get('tailnet.mode')).toBe('system');
    expect(screen.queryByText('撤销')).toBeNull();
  });

  it('can dismiss the embedded explanation without switching', async () => {
    resetTailnetSwitchStore();
    const { settingsStore } = await renderHome({
      core: createFakeCore({ tailnetAvailable: true, network: netCompanyVpn }),
      settings: { networks: [COMPANY_NETWORK] },
    });

    await fireEvent.press(screen.getByTestId('home-banner-vpnConflict-why'));
    await fireEvent.press(screen.getByTestId('home-vpn-info-later'));
    expect(screen.queryByTestId('home-vpn-info')).toBeNull();
    expect(settingsStore.get('tailnet.mode')).toBe('system');

    await fireEvent.press(screen.getByTestId('home-banner-vpnConflict-why'));
    await fireEvent.press(screen.getByTestId('home-vpn-info-use-embedded'));
    expect(screen.queryByTestId('home-vpn-info')).toBeNull();
    expect(screen.getByTestId('tailnet-control-url-sheet')).toBeOnTheScreen();
  });

  it('shows the Android VPN explanation sheet and can open Tailscale', async () => {
    const platform = new FakePlatform();
    const core = createFakeCore({ network: netCompanyVpn });
    await renderHome({
      core,
      platform,
      settings: { networks: [COMPANY_NETWORK] },
    });

    expect(
      screen.getByText(
        'Android 同时只能开一个 VPN · 先断开公司 VPN，再打开 Tailscale',
      ),
    ).toBeOnTheScreen();
    expect(
      screen.queryByText(
        'iOS 同时只能开一个系统 VPN · 先断开公司 VPN，再打开 Tailscale',
      ),
    ).toBeNull();

    await fireEvent.press(screen.getByTestId('home-banner-vpnConflict-action'));
    expect(screen.getByTestId('home-vpn-info')).toBeOnTheScreen();
    expect(
      screen.getByText(
        'Android 同一时间只能运行一个 VPN。公司 VPN 连上后，系统会断开 Tailscale。',
      ),
    ).toBeOnTheScreen();

    await fireEvent.press(screen.getByTestId('home-vpn-info-got-it'));
    expect(screen.queryByTestId('home-vpn-info')).toBeNull();

    await fireEvent.press(screen.getByTestId('home-banner-vpnConflict-action'));
    await fireEvent.press(screen.getByTestId('home-vpn-info-open-tailscale'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(platform.opened[0]).toBe('tailscale://');
  });

  it('ignores a work-profile VPN conflict', async () => {
    const workProfileNetwork: NetworkStatus = {
      ...netCompanyVpn,
      ifaces: netCompanyVpn.ifaces.map(iface => ({
        ...iface,
        workProfile: iface.isVpn,
      })),
    };
    await renderHome({
      core: createFakeCore({ network: workProfileNetwork }),
      settings: { networks: [COMPANY_NETWORK] },
    });

    expect(screen.queryByTestId('home-banner-vpnConflict')).toBeNull();
    expect(screen.getByText('Tailscale 没有连接')).toBeOnTheScreen();
  });

  it('falls back through Android Tailscale URLs', async () => {
    const platform = new FakePlatform();
    platform.openUrlResult = false;
    await renderHome({
      core: createFakeCore({ network: netNoTailnet }),
      platform,
    });

    await fireEvent.press(screen.getByTestId('home-banner-tailnetDown-action'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(platform.opened).toEqual([
      'tailscale://',
      'market://details?id=com.tailscale.ipn',
      'https://play.google.com/store/apps/details?id=com.tailscale.ipn',
    ]);
  });

  it('uses Android hit targets and announces status through a live region', async () => {
    await renderHome();

    expect(
      StyleSheet.flatten(screen.getByTestId('home-settings').props.style),
    ).toMatchObject({
      width: hit.min,
      height: hit.min,
      borderRadius: hit.min / 2,
    });
    const header = screen.getByTestId('home-settings').parent?.parent;
    expect(StyleSheet.flatten(header?.props.style)).toMatchObject({
      minHeight: 64,
    });
    // Studio and the office PC are the paired computers online.
    expect(
      screen.getByText('Tailscale · 2 台在线').props.accessibilityLiveRegion,
    ).toBe('polite');
  });
});

describe('ios', () => {
  beforeEach(() => {
    jest.replaceProperty(Platform, 'OS', 'ios');
  });

  it('announces a tone change once without announcing initial status', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const core = createFakeCore();
    await renderHome({ core });

    expect(announce).not.toHaveBeenCalled();

    await act(async () => {
      core.emit({ type: 'network.changed', status: netNoTailnet });
    });

    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce).toHaveBeenCalledWith('Tailscale 已断开');
  });

  it('announces once when tailnet state precedes the network update', async () => {
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

    await fireEvent.changeText(screen.getByTestId('home-search'), 'studio');

    expect(announce.mock.calls.slice(initialCallCount)).toEqual([
      ['Tailscale 已断开'],
    ]);
  });
});
