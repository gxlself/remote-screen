import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { TabScreenProps } from '../../app/navigation/routes';
import { qk } from '../../features/data/queryClient';
import type { SyncNs } from '../../features/data/localSync';
import { MemoryKv } from '../../features/storage/kv';
import { resetTailnetSwitchStore } from '../../features/tailnet/switchStore';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import {
  netCompanyVpn,
  netNoTailnet,
  netTailnetOk,
  routeEmbeddedNeedsLogin,
  routeEmbeddedRunning,
  tailnetNeedsLogin,
} from '../../native/rscore/fixtures';
import type {
  SyncValue,
  TailscaleApiStatus,
  TailscaleDevice,
} from '../../native/rscore/types';
import { mockNavigation } from '../../test/navigation';
import { seedLocalSync } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import ToastHost, { hideToast } from '../../components/ToastHost';
import { networks as copy } from '../../i18n/zh/networks';
import { tailscaleApi } from '../../i18n/zh/tailscaleApi';
import NetworkSections from './NetworkSections';

const queryClients: QueryClient[] = [];

const companyNetwork = {
  id: 'n1',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

async function renderSections(opts?: {
  network?: typeof netTailnetOk;
  /** Synced namespaces already saved on this phone. */
  sync?: Partial<Record<SyncNs, SyncValue>>;
  core?: FakeCore;
  settings?: Parameters<typeof renderWithProviders>[1] extends infer O
    ? O extends { settings?: infer S }
      ? S
      : never
    : never;
}) {
  const core =
    opts?.core ??
    createFakeCore({
      network: opts?.network ?? netTailnetOk,
    });
  const kv = new MemoryKv();
  for (const [ns, saved] of Object.entries(opts?.sync ?? {})) {
    seedLocalSync(kv, ns as SyncNs, saved);
  }
  const navigation =
    mockNavigation<'Networks'>() as unknown as TabScreenProps<'Settings'>['navigation'];
  const result = await renderWithProviders(
    <>
      <NetworkSections navigation={navigation} />
      <ToastHost />
    </>,
    // These tests describe the Tailscale block, which needs 外网访问 on.
    { core, kv, settings: { 'tailnet.enabled': true, ...opts?.settings } },
  );
  queryClients.push(result.queryClient);
  await act(() => Promise.resolve());
  return { ...result, core, navigation };
}

beforeEach(() => {
  resetTailnetSwitchStore();
});
afterEach(async () => {
  jest.restoreAllMocks();
  await act(async () => hideToast());
  await act(() => Promise.resolve());
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
});

it('shows connected and disconnected tailnet states', async () => {
  const { unmount } = await renderSections({ network: netTailnetOk });
  expect(screen.getByText('已连接')).toBeOnTheScreen();
  await unmount();

  await renderSections({ network: netNoTailnet });
  expect(screen.getByText('未连接')).toBeOnTheScreen();
});

it('shows detecting when the network status is unknown', async () => {
  const core = createFakeCore();
  core.failNext('network.status', new Error('network unavailable'));
  const result = await renderWithProviders(
    <NetworkSections
      navigation={
        mockNavigation<'Networks'>() as unknown as TabScreenProps<'Settings'>['navigation']
      }
    />,
    { core, settings: { 'tailnet.enabled': true } },
  );
  queryClients.push(result.queryClient);
  expect(screen.getByText('检测中')).toBeOnTheScreen();
});

it('shows the one available mode as plain text when App 内连接 is missing', async () => {
  const { settingsStore } = await renderSections();
  // S05: a mode this phone cannot use is not offered at all.
  expect(screen.queryByLabelText('App 内连接')).toBeNull();
  expect(screen.queryByLabelText('接入 Tailscale 的方式')).toBeNull();
  expect(screen.getByText('接入方式')).toBeOnTheScreen();
  expect(screen.getByText('Tailscale App')).toBeOnTheScreen();
  expect(settingsStore.get('tailnet.mode')).toBe('system');
});

it('hides every in-app tailnet entry in a build without it', async () => {
  // Settings an earlier build with the in-app tailnet left behind.
  await renderSections({
    network: netNoTailnet,
    settings: {
      'tailnet.mode': 'embedded',
      'tailnet.controlUrl': 'https://hs.example.com',
    },
  });

  expect(screen.queryByText(/App 内/)).toBeNull();
  expect(screen.queryByText(/Headscale/)).toBeNull();
  expect(screen.queryByText(copy.tailnet.loginTitle)).toBeNull();
  expect(screen.queryByText(copy.tailnet.embeddedHint)).toBeNull();
  expect(screen.getByText(copy.tailnet.systemHint)).toBeOnTheScreen();
  expect(screen.getByText(copy.tailnet.downHint)).toBeOnTheScreen();
  // The Tailscale API token is not networking and stays.
  expect(screen.getByText(tailscaleApi.row.title)).toBeOnTheScreen();
});

it('turns prewarm off from the switch', async () => {
  const { settingsStore } = await renderSections();
  await fireEvent.press(screen.getByLabelText('提前建立连接'));
  expect(settingsStore.get('prewarm.enabled')).toBe(false);
});

it('lists a registered network with its VPN state and opens the editor', async () => {
  const { navigation } = await renderSections({
    network: netCompanyVpn,
    settings: { networks: [companyNetwork] },
  });
  expect(screen.getByText('公司网络')).toBeOnTheScreen();
  expect(
    screen.getByText('内网 10.30.0.0/16 · VPN 10.99.0.0/16'),
  ).toBeOnTheScreen();
  expect(screen.getByText('VPN 已连接')).toBeOnTheScreen();
  // S14: the row says it opens the list, and does.
  const row = screen.getByText('公司网络');
  await fireEvent.press(row);
  expect(navigation.navigate).toHaveBeenCalledWith('Networks');
  expect(screen.queryByLabelText('编辑 公司网络')).toBeNull();
  expect(screen.getByRole('button', { name: /公司网络/ })).toHaveProp(
    'accessibilityHint',
    '打开其他网络列表',
  );
});

it('offers to add a network when none are registered', async () => {
  await renderSections();
  expect(screen.getByText('添加其他网络')).toBeOnTheScreen();
});

it('counts manual computers and opens AddByAddress', async () => {
  const { navigation } = await renderSections({
    sync: {
      servers: {
        version: 1,
        value: {
          v: 1,
          entries: [
            {
              id: 'c1',
              kind: 'computer',
              host: '10.30.0.5',
              port: 7400,
              hostId: null,
              name: null,
              networkId: null,
              addedAtUs: 1,
            },
          ],
          tailscaleSsh: {},
        },
      },
    },
  });
  expect(screen.getByText('1 台')).toBeOnTheScreen();
  await fireEvent.press(
    screen.getByLabelText('按 IP 或主机名添加的设备，1 台'),
  );
  expect(navigation.navigate).toHaveBeenCalledWith('AddByAddress');
});

it('turns off local preference from the switch', async () => {
  const { settingsStore } = await renderSections();
  await fireEvent.press(screen.getByLabelText('优先走内网和局域网'));
  expect(settingsStore.get('net.preferLocal')).toBe(false);
});

it('turns off the VPN conflict reminder from the switch', async () => {
  const { settingsStore } = await renderSections();
  await fireEvent.press(
    screen.getByLabelText('公司 VPN 顶掉 Tailscale 时提醒'),
  );
  expect(settingsStore.get('net.warnVpnConflict')).toBe(false);
});

it('shows all three modes only when embedded tailnet is available', async () => {
  const embedded = await renderSections({
    core: createFakeCore({ tailnetAvailable: true }),
  });
  expect(screen.getByLabelText('自动')).toBeOnTheScreen();
  await embedded.unmount();

  await renderSections();
  expect(screen.queryByLabelText('自动')).toBeNull();
});

it('offers only the Tailscale App when the probe fails', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  core.failNext('tailnet.available', new Error('unsupported'));
  const { settingsStore } = await renderSections({ core });
  expect(screen.queryByLabelText('自动')).toBeNull();
  expect(screen.queryByLabelText('App 内连接')).toBeNull();
  expect(screen.getByText('Tailscale App')).toBeOnTheScreen();
  expect(settingsStore.get('tailnet.mode')).toBe('system');
});

it('holds the App-only modes while the probe runs so the row keeps its size', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  let resolve: (value: boolean) => void = () => {};
  jest.spyOn(core.tailnet, 'available').mockImplementation(
    () =>
      new Promise(done => {
        resolve = done;
      }),
  );
  await renderSections({ core });
  // S06: the three options are there, but only the Tailscale App works yet.
  expect(screen.getByLabelText('App 内连接')).toBeDisabled();
  expect(screen.getByLabelText('自动')).toBeDisabled();
  expect(screen.getByLabelText('Tailscale App')).not.toBeDisabled();
  await act(async () => resolve(true));
  expect(screen.getByLabelText('App 内连接')).not.toBeDisabled();
});

it('shows a spinner and holds the modes while switching', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const { settingsStore } = await renderSections({
    core,
    settings: { 'tailnet.controlUrl': 'https://hs.example.com' },
  });
  let resolve: (value: boolean) => void = () => {};
  jest.spyOn(core.tailnet, 'available').mockImplementation(
    () =>
      new Promise(done => {
        resolve = done;
      }),
  );
  await fireEvent.press(screen.getByLabelText('App 内连接'));
  // S05: a switch in flight shows progress and blocks a second tap.
  expect(screen.getByTestId('tailnet-mode-switching')).toBeOnTheScreen();
  expect(screen.getByLabelText('自动')).toBeDisabled();
  await act(async () => {
    resolve(true);
    await Promise.resolve();
    await Promise.resolve();
  });
  await waitFor(() =>
    expect(screen.queryByTestId('tailnet-mode-switching')).toBeNull(),
  );
  expect(settingsStore.get('tailnet.mode')).toBe('embedded');
});

it('toasts when the switch to App 内连接 fails', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const { settingsStore } = await renderSections({ core });
  jest.spyOn(core.tailnet, 'available').mockResolvedValue(false);
  await fireEvent.press(screen.getByLabelText('App 内连接'));
  await waitFor(() =>
    expect(screen.getByTestId('global-toast')).toHaveTextContent(
      copy.tailnet.switchFailed,
    ),
  );
  expect(settingsStore.get('tailnet.mode')).toBe('system');
});

it('prompts for a control URL before switching to embedded mode', async () => {
  const { settingsStore } = await renderSections({
    core: createFakeCore({ tailnetAvailable: true }),
  });
  await fireEvent.press(screen.getByLabelText('App 内连接'));
  expect(screen.getByTestId('tailnet-control-url-sheet')).toBeOnTheScreen();
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByRole('button', { name: '继续' }));
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(settingsStore.get('tailnet.mode')).toBe('embedded');
  expect(settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
  expect(screen.queryByTestId('tailnet-control-url-sheet')).toBeNull();
});

it('switches directly with an existing URL and shows its row and hint', async () => {
  const { settingsStore } = await renderSections({
    core: createFakeCore({ tailnetAvailable: true }),
    settings: { 'tailnet.controlUrl': 'https://hs.example.com' },
  });
  await fireEvent.press(screen.getByLabelText('App 内连接'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(settingsStore.get('tailnet.mode')).toBe('embedded');
  expect(screen.queryByTestId('tailnet-control-url-sheet')).toBeNull();
  expect(screen.getByText(copy.tailnet.controlUrlTitle)).toBeOnTheScreen();
  expect(screen.getByText('hs.example.com')).toBeOnTheScreen();
  expect(screen.getByText(copy.tailnet.embeddedHint)).toBeOnTheScreen();
});

it('asks for an address before automatic mode and sets it once saved', async () => {
  const { settingsStore } = await renderSections({
    core: createFakeCore({ tailnetAvailable: true }),
  });
  await fireEvent.press(screen.getByLabelText('自动'));
  await act(async () => {
    await Promise.resolve();
  });
  // Automatic mode without an address would do nothing; ask first.
  expect(settingsStore.get('tailnet.mode')).toBe('system');
  expect(screen.getByRole('button', { name: '保存' })).toBeOnTheScreen();

  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(settingsStore.get('tailnet.mode')).toBe('auto');
  expect(settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
  expect(screen.getByText(copy.tailnet.autoHint)).toBeOnTheScreen();
  expect(screen.getByTestId('global-toast')).toHaveTextContent(
    copy.tailnet.saved,
  );
});

it('keeps the mode when the automatic-mode sheet is closed without an address', async () => {
  const { settingsStore } = await renderSections({
    core: createFakeCore({ tailnetAvailable: true }),
  });
  await fireEvent.press(screen.getByLabelText('自动'));
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(settingsStore.get('tailnet.mode')).toBe('system');
  expect(screen.getByTestId('global-toast')).toHaveTextContent(
    copy.tailnet.autoNeedsUrl,
  );
});

it('points to the Tailscale App when the phone is off the tailnet', async () => {
  await renderSections({ network: netNoTailnet });
  // S17: the down state says what to do next.
  expect(screen.getByText(copy.tailnet.downHint)).toBeOnTheScreen();
});

it('reads each switch description as its hint', async () => {
  await renderSections();
  // S22
  expect(screen.getByLabelText('提前建立连接')).toHaveProp(
    'accessibilityHint',
    copy.prewarm.subtitle,
  );
  expect(screen.getByLabelText('优先走内网和局域网')).toHaveProp(
    'accessibilityHint',
    copy.prefs.preferLocal.subtitle,
  );
});

it('shows the embedded route state and opens its login URL', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const { unmount } = await renderSections({ core });
  await act(() => core.setTailnet({ route: routeEmbeddedRunning }));
  expect(screen.getByText('已连接 · App 内')).toBeOnTheScreen();
  await unmount();

  const loginCore = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetNeedsLogin,
  });
  const login = await renderSections({ core: loginCore });
  await act(() => loginCore.setTailnet({ route: routeEmbeddedNeedsLogin }));
  expect(screen.getByText('待登录')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText(copy.tailnet.loginTitle));
  await act(async () => {
    await Promise.resolve();
  });
  expect(login.platform.opened).toEqual([
    'https://hs.example.com/register/nodekey-test',
  ]);
  expect(screen.queryByTestId('global-toast')).toBeNull();
});

it('toasts when the login page cannot be opened', async () => {
  const core = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetNeedsLogin,
  });
  const login = await renderSections({ core });
  await act(() => core.setTailnet({ route: routeEmbeddedNeedsLogin }));
  login.platform.openUrlResult = false;
  await fireEvent.press(screen.getByText(copy.tailnet.loginTitle));
  await waitFor(() =>
    expect(screen.getByTestId('global-toast')).toHaveTextContent(
      copy.tailnet.loginFailed,
    ),
  );
});

it('shows neither a Hub row nor remote wake, even with a stored Hub address', async () => {
  const kv = new MemoryKv();
  kv.setJSON('settings.hub.url', 'http://rs-hub.tailnet.example:7300');
  const core = createFakeCore({ network: netTailnetOk });
  const navigation =
    mockNavigation<'Networks'>() as unknown as TabScreenProps<'Settings'>['navigation'];
  const result = await renderWithProviders(
    <NetworkSections navigation={navigation} />,
    { core, kv, settings: { 'tailnet.enabled': true } },
  );
  queryClients.push(result.queryClient);
  await act(() => Promise.resolve());

  expect(screen.getByText(copy.tailnet.title)).toBeOnTheScreen();
  expect(screen.getByText(copy.prewarm.title)).toBeOnTheScreen();
  expect(screen.queryByText(/Hub/)).toBeNull();
  expect(screen.queryByText('rs-hub.tailnet.example')).toBeNull();
  expect(screen.queryByText('远程唤醒')).toBeNull();
});

describe('Tailscale API token', () => {
  const TOKEN = 'tskey-api-kFAKE1CNTRL-fakefakefakefakefake';
  const devices: TailscaleDevice[] = ['aliyun-bj', 'alice-mac', 'bob-pc'].map(
    (name, index) => ({
      id: `n${index}`,
      name,
      dnsName: `${name}.tail1234.ts.net`,
      os: (['linux', 'macos', 'windows'] as const)[index] ?? 'linux',
      addresses: [`100.64.0.${60 + index}`],
      online: true,
      lastSeenAt: null,
      tags: [],
    }),
  );

  function tsCore(configured = false) {
    const core = createFakeCore({ network: netTailnetOk });
    core.setTailscale({ configured, devices });
    return core;
  }

  function apiOf(core: FakeCore) {
    const api = core.tailscaleApi;
    if (api === undefined) {
      throw new Error('tailscaleApi missing');
    }
    return api;
  }

  async function openSheet() {
    await fireEvent.press(screen.getByTestId('tailscale-api-row'));
    expect(screen.getByTestId('tailscale-api-sheet')).toBeOnTheScreen();
  }

  function calls(core: FakeCore, method: string): unknown[][] {
    return core.calls.filter(call => call.method === method).map(c => c.args);
  }

  it('hides the entry when the native package has no Tailscale API', async () => {
    const core = tsCore();
    core.tailscaleApi = undefined;
    await renderSections({ core });
    expect(screen.queryByTestId('tailscale-api-row')).toBeNull();
    expect(screen.queryByText(tailscaleApi.row.title)).toBeNull();
  });

  it('shows the entry with its status in the access section', async () => {
    await renderSections({ core: tsCore() });
    await waitFor(() =>
      expect(
        screen.getByLabelText('Tailscale 设备列表，未设置'),
      ).toBeOnTheScreen(),
    );
  });

  it('says the status is loading or unreadable instead of 未设置', async () => {
    const core = tsCore();
    let fail: (error: Error) => void = () => undefined;
    jest.spyOn(apiOf(core), 'status').mockImplementation(
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    await renderSections({ core });
    // S12
    expect(
      screen.getByLabelText(`Tailscale 设备列表，${tailscaleApi.row.loading}`),
    ).toBeOnTheScreen();
    await act(async () => fail(new Error('keychain locked')));
    await waitFor(() =>
      expect(
        screen.getByLabelText(
          `Tailscale 设备列表，${tailscaleApi.row.loadFailed}`,
        ),
      ).toBeOnTheScreen(),
    );
  });

  it('titles the sheet as a token and links to the admin console', async () => {
    const { platform } = await renderSections({ core: tsCore() });
    await openSheet();
    // S26
    expect(screen.getByText(tailscaleApi.sheet.title)).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('tailscale-api-admin'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(platform.opened).toEqual([
      'https://login.tailscale.com/admin/settings/oauth',
    ]);
  });

  it.each([
    ['auth', '令牌无效或已过期'],
    ['network', '连不上 Tailscale'],
    ['http', '请求失败'],
  ] as const)('shows lastError %s in the row', async (lastError, text) => {
    const core = tsCore(true);
    const status: TailscaleApiStatus = {
      configured: true,
      controlUrl: null,
      lastError,
      lastOkAt: null,
      deviceCount: null,
    };
    jest.spyOn(apiOf(core), 'status').mockResolvedValue(status);
    await renderSections({ core });
    await waitFor(() => expect(screen.getByText(text)).toBeOnTheScreen());
  });

  it('saves the trimmed token, toasts the count and refreshes the directory', async () => {
    const core = tsCore();
    const { kv, queryClient } = await renderSections({ core });
    const invalidate = jest.spyOn(queryClient, 'invalidateQueries');
    await openSheet();

    const input = screen.getByTestId('tailscale-api-token');
    await fireEvent.changeText(input, `  ${TOKEN}\n`);
    await fireEvent.press(screen.getByTestId('tailscale-api-save'));

    await waitFor(() =>
      expect(screen.getByText('找到 3 台设备')).toBeOnTheScreen(),
    );
    expect(calls(core, 'tailscaleApi.setToken')).toEqual([[TOKEN, null]]);
    expect(screen.queryByTestId('tailscale-api-sheet')).toBeNull();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.directory });
    await waitFor(() =>
      expect(screen.getByText('已连接 · 3 台设备')).toBeOnTheScreen(),
    );

    // The token only went to setToken: not to kv, logs or the query caches.
    const others = core.calls.filter(
      call => call.method !== 'tailscaleApi.setToken',
    );
    expect(JSON.stringify(others)).not.toContain(TOKEN);
    expect(kv.keys().some(key => kv.getString(key)?.includes(TOKEN))).toBe(
      false,
    );
    const cached = queryClient
      .getQueryCache()
      .getAll()
      .map(query => query.state.data);
    expect(JSON.stringify(cached)).not.toContain(TOKEN);
    expect(queryClient.getMutationCache().getAll()).toHaveLength(0);
  });

  it('keeps the sheet and the token on a 401', async () => {
    const core = tsCore();
    core.failNext(
      'tailscaleApi.setToken',
      new RsCoreError('http', 'tailscale api unauthorized', 401),
    );
    await renderSections({ core });
    await openSheet();
    await fireEvent.changeText(
      screen.getByTestId('tailscale-api-token'),
      'tskey-api-expired',
    );
    await fireEvent.press(screen.getByTestId('tailscale-api-save'));

    await waitFor(() =>
      expect(
        screen.getByText('令牌无效或已过期，或缺少设备读取权限'),
      ).toBeOnTheScreen(),
    );
    expect(screen.getByTestId('tailscale-api-sheet')).toBeOnTheScreen();
    expect(screen.getByTestId('tailscale-api-token')).toHaveProp(
      'value',
      'tskey-api-expired',
    );
    expect(screen.queryByText('找到 3 台设备')).toBeNull();
  });

  it('shows the network message when api.tailscale.com is unreachable', async () => {
    const core = tsCore();
    core.failNext(
      'tailscaleApi.setToken',
      new RsCoreError('network', 'connect timed out'),
    );
    await renderSections({ core });
    await openSheet();
    await fireEvent.changeText(
      screen.getByTestId('tailscale-api-token'),
      TOKEN,
    );
    await fireEvent.press(screen.getByTestId('tailscale-api-save'));
    await waitFor(() =>
      expect(
        screen.getByText(tailscaleApi.sheet.errors.network),
      ).toBeOnTheScreen(),
    );
  });

  it('sends the token once while saving and ignores a blank one', async () => {
    const core = tsCore();
    let finish: (value: TailscaleDevice[]) => void = () => undefined;
    const setToken = jest.spyOn(apiOf(core), 'setToken').mockImplementation(
      () =>
        new Promise(resolve => {
          finish = resolve;
        }),
    );
    await renderSections({ core });
    await openSheet();

    const input = screen.getByTestId('tailscale-api-token');
    await fireEvent.changeText(input, '   ');
    await fireEvent.press(screen.getByTestId('tailscale-api-save'));
    expect(setToken).not.toHaveBeenCalled();

    await fireEvent.changeText(input, TOKEN);
    await fireEvent.press(screen.getByTestId('tailscale-api-save'));
    await fireEvent.press(screen.getByTestId('tailscale-api-save'));
    expect(setToken).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('tailscale-api-save-spinner')).toBeOnTheScreen();

    await act(async () => {
      finish(devices);
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(screen.queryByTestId('tailscale-api-sheet')).toBeNull(),
    );
  });

  it('marks the token field secure and keeps it out of autofill', async () => {
    await renderSections({ core: tsCore() });
    await openSheet();
    const input = screen.getByTestId('tailscale-api-token');
    expect(input).toHaveProp('secureTextEntry', true);
    expect(input).toHaveProp('autoCapitalize', 'none');
    expect(input).toHaveProp('autoCorrect', false);
    expect(input).toHaveProp('autoComplete', 'off');
    expect(input).toHaveProp('textContentType', 'none');
    expect(screen.getByLabelText(tailscaleApi.sheet.tokenLabel)).toBe(input);
  });

  it('drops the typed token when the sheet closes', async () => {
    await renderSections({ core: tsCore() });
    await openSheet();
    await fireEvent.changeText(
      screen.getByTestId('tailscale-api-token'),
      TOKEN,
    );
    await fireEvent.press(screen.getByTestId('tailscale-api-sheet-close'));
    expect(screen.queryByTestId('tailscale-api-sheet')).toBeNull();

    await openSheet();
    expect(screen.getByTestId('tailscale-api-token')).toHaveProp('value', '');
  });

  it('refreshes the devices of a saved token', async () => {
    const core = tsCore(true);
    const { queryClient } = await renderSections({ core });
    const invalidate = jest.spyOn(queryClient, 'invalidateQueries');
    await openSheet();
    expect(screen.getByLabelText('当前状态，已设置')).toBeOnTheScreen();

    await fireEvent.press(screen.getByTestId('tailscale-api-refresh'));
    await waitFor(() =>
      expect(
        screen.getByLabelText('当前状态，已连接 · 3 台设备'),
      ).toBeOnTheScreen(),
    );
    expect(calls(core, 'tailscaleApi.devices')).toEqual([[true]]);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.directory });
    expect(screen.getByTestId('tailscale-api-sheet')).toBeOnTheScreen();
    // S26: the refresh says what it found without closing the sheet.
    expect(
      screen.getByText('已刷新，找到 3 台设备', {
        includeHiddenElements: true,
      }),
    ).toBeOnTheScreen();
  });

  it('removes a saved token after confirming', async () => {
    const core = tsCore(true);
    const { queryClient } = await renderSections({ core });
    const invalidate = jest.spyOn(queryClient, 'invalidateQueries');
    await openSheet();

    await fireEvent.press(screen.getByRole('button', { name: '移除令牌' }));
    // Like the network editor, the dialog sits beside the modal sheet.
    const hidden = { includeHiddenElements: true };
    expect(
      screen.getByTestId('tailscale-api-remove-dialog-card', hidden),
    ).toBeOnTheScreen();
    expect(screen.getByText('移除 Tailscale 令牌？', hidden)).toBeOnTheScreen();
    expect(calls(core, 'tailscaleApi.clearToken')).toHaveLength(0);
    await fireEvent.press(
      screen.getByRole('button', { name: '移除', ...hidden }),
    );

    await waitFor(() =>
      expect(screen.getByText('已移除 Tailscale 令牌')).toBeOnTheScreen(),
    );
    expect(calls(core, 'tailscaleApi.clearToken')).toHaveLength(1);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.directory });
    expect(screen.queryByTestId('tailscale-api-sheet')).toBeNull();
    await waitFor(() =>
      expect(
        screen.getByLabelText('Tailscale 设备列表，未设置'),
      ).toBeOnTheScreen(),
    );
  });

  it('offers refresh and remove only once a token is saved', async () => {
    await renderSections({ core: tsCore() });
    await openSheet();
    expect(screen.queryByTestId('tailscale-api-refresh')).toBeNull();
    expect(screen.queryByTestId('tailscale-api-remove')).toBeNull();
  });

  describe('Headscale', () => {
    const KEY = 'hskey-api-FAKE.fakefakefake';

    async function chooseHeadscale() {
      await openSheet();
      expect(screen.queryByTestId('tailscale-api-control-url')).toBeNull();
      await fireEvent.press(screen.getByTestId('tailscale-api-kind-headscale'));
    }

    it('asks for the control server once Headscale is picked', async () => {
      await renderSections({ core: tsCore() });
      await chooseHeadscale();

      const url = screen.getByTestId('tailscale-api-control-url');
      expect(url).toHaveProp('keyboardType', 'url');
      expect(url).toHaveProp('autoCapitalize', 'none');
      expect(url).toHaveProp('autoCorrect', false);
      expect(url).toHaveProp('placeholder', 'https://hs.example.com');
      expect(screen.getByLabelText('控制服务器')).toBe(url);
      expect(
        screen.getByText(tailscaleApi.sheet.headscale.apiKey),
      ).toBeOnTheScreen();
      expect(screen.queryByText(tailscaleApi.sheet.oauth)).toBeNull();
      const key = screen.getByTestId('tailscale-api-token');
      expect(key).toHaveProp('secureTextEntry', true);
      expect(screen.getByLabelText('Headscale API key')).toBe(key);

      await fireEvent.press(screen.getByTestId('tailscale-api-kind-tailscale'));
      expect(screen.queryByTestId('tailscale-api-control-url')).toBeNull();
      expect(screen.getByText(tailscaleApi.sheet.oauth)).toBeOnTheScreen();
    });

    it('saves the trimmed key and server and names the server in the row', async () => {
      const core = tsCore();
      const { kv, queryClient } = await renderSections({ core });
      await chooseHeadscale();

      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-control-url'),
        '  https://hs.example.com/ ',
      );
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-token'),
        `\t${KEY}  `,
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-save'));

      await waitFor(() =>
        expect(screen.getByText('找到 3 台设备')).toBeOnTheScreen(),
      );
      expect(calls(core, 'tailscaleApi.setToken')).toEqual([
        [KEY, 'https://hs.example.com/'],
      ]);
      await waitFor(() =>
        expect(
          screen.getByLabelText(
            'Tailscale 设备列表，hs.example.com · 已连接 · 3 台设备',
          ),
        ).toBeOnTheScreen(),
      );
      const others = core.calls.filter(
        call => call.method !== 'tailscaleApi.setToken',
      );
      expect(JSON.stringify(others)).not.toContain(KEY);
      expect(kv.keys().some(k => kv.getString(k)?.includes(KEY))).toBe(false);
      const cached = queryClient
        .getQueryCache()
        .getAll()
        .map(query => query.state.data);
      expect(JSON.stringify(cached)).not.toContain(KEY);
    });

    it('sends null again after switching back to Tailscale', async () => {
      const core = tsCore();
      await renderSections({ core });
      await chooseHeadscale();
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-control-url'),
        'https://hs.example.com',
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-kind-tailscale'));
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-token'),
        TOKEN,
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-save'));
      await waitFor(() =>
        expect(calls(core, 'tailscaleApi.setToken')).toEqual([[TOKEN, null]]),
      );
    });

    it('needs a control server before saving', async () => {
      const core = tsCore();
      await renderSections({ core });
      await chooseHeadscale();
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-token'),
        KEY,
      );
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-control-url'),
        '   ',
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-save'));
      expect(calls(core, 'tailscaleApi.setToken')).toEqual([]);
    });

    it('shows the bad control url error from Rust and keeps the sheet', async () => {
      const core = tsCore();
      await renderSections({ core });
      await chooseHeadscale();
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-control-url'),
        'http://hs.example.com',
      );
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-token'),
        KEY,
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-save'));

      await waitFor(() =>
        expect(screen.getByText('控制服务器地址不对')).toBeOnTheScreen(),
      );
      expect(calls(core, 'tailscaleApi.setToken')).toEqual([
        [KEY, 'http://hs.example.com'],
      ]);
      expect(screen.getByTestId('tailscale-api-sheet')).toBeOnTheScreen();
      expect(screen.getByTestId('tailscale-api-control-url')).toHaveProp(
        'value',
        'http://hs.example.com',
      );
      expect(await apiOf(core).status()).toMatchObject({ configured: false });
    });

    it('uses the Headscale wording for a rejected key', async () => {
      const core = tsCore();
      core.failNext(
        'tailscaleApi.setToken',
        new RsCoreError('http', 'tailscale api unauthorized', 401),
      );
      await renderSections({ core });
      await chooseHeadscale();
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-control-url'),
        'https://hs.example.com',
      );
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-token'),
        KEY,
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-save'));
      await waitFor(() =>
        expect(screen.getByText('API key 无效或已过期')).toBeOnTheScreen(),
      );
    });

    it('opens on Headscale with the saved server', async () => {
      const core = tsCore(true);
      core.setTailscale({ controlUrl: 'https://hs.example.com' });
      await renderSections({ core });
      await waitFor(() =>
        expect(
          screen.getByLabelText('Tailscale 设备列表，hs.example.com · 已设置'),
        ).toBeOnTheScreen(),
      );
      await openSheet();

      expect(screen.getByTestId('tailscale-api-kind-headscale')).toBeSelected();
      expect(
        screen.getByTestId('tailscale-api-kind-tailscale'),
      ).not.toBeSelected();
      expect(screen.getByTestId('tailscale-api-control-url')).toHaveProp(
        'value',
        'https://hs.example.com',
      );
      expect(
        screen.getByLabelText('当前状态，hs.example.com · 已设置'),
      ).toBeOnTheScreen();

      // Pasting a new key keeps the saved server.
      await fireEvent.changeText(
        screen.getByTestId('tailscale-api-token'),
        KEY,
      );
      await fireEvent.press(screen.getByTestId('tailscale-api-save'));
      await waitFor(() =>
        expect(calls(core, 'tailscaleApi.setToken')).toEqual([
          [KEY, 'https://hs.example.com'],
        ]),
      );
    });
  });
});

describe('with 外网访问 (Tailscale) switched off', () => {
  it('shows a single switch and none of the Tailscale settings', async () => {
    await renderSections({
      network: netCompanyVpn,
      settings: { 'tailnet.enabled': false, networks: [companyNetwork] },
    });

    expect(screen.getByText(copy.outside.title)).toBeOnTheScreen();
    expect(screen.getByText(copy.outside.subtitle)).toBeOnTheScreen();
    expect(screen.queryByText(copy.tailnet.title)).toBeNull();
    expect(screen.queryByText(copy.tailnet.modeTitle)).toBeNull();
    expect(screen.queryByText(copy.tailnet.systemHint)).toBeNull();
    expect(screen.queryByText(copy.tailnet.downHint)).toBeNull();
    expect(screen.queryByText(tailscaleApi.row.title)).toBeNull();
    expect(
      screen.queryByLabelText(copy.prefs.warnVpnConflict.title),
    ).toBeNull();
    expect(screen.getByText(copy.prefs.preferLocal.subtitleLan)).toBeOnTheScreen();
    expect(screen.getByText(copy.footerLan)).toBeOnTheScreen();
    // Pre-warming is not a Tailscale feature and stays.
    expect(screen.getByText(copy.prewarm.title)).toBeOnTheScreen();
    // The switch's own title is the only place Tailscale is named.
    expect(screen.queryAllByText(/Tailscale|Headscale|tailnet/)).toHaveLength(1);
  });

  it('reveals the Tailscale block once switched on', async () => {
    const { settingsStore } = await renderSections({
      network: netTailnetOk,
      settings: { 'tailnet.enabled': false },
    });

    await fireEvent.press(screen.getByLabelText(copy.outside.title));
    expect(settingsStore.get('tailnet.enabled')).toBe(true);
    expect(await screen.findByText(copy.tailnet.title)).toBeOnTheScreen();
    expect(screen.getByText(copy.tailnet.up)).toBeOnTheScreen();
    expect(
      screen.getByLabelText(copy.prefs.warnVpnConflict.title),
    ).toBeOnTheScreen();
    expect(screen.getByText(copy.footer)).toBeOnTheScreen();

    await fireEvent.press(screen.getByLabelText(copy.outside.title));
    expect(settingsStore.get('tailnet.enabled')).toBe(false);
    expect(screen.queryByText(copy.tailnet.title)).toBeNull();
  });
});
