import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import AddByAddressScreen from '.';
import type { RootScreenProps } from '../../app/navigation/routes';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore, FakeFixtures } from '../../native/rscore/fakeCore';
import { netCompanyVpn, pairedHosts } from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import type { NetworkEntry } from '../../features/settings/schema';
import { localSyncGet } from '../../features/data/localSync';
import { MemoryKv } from '../../features/storage/kv';
import { addByAddress } from '../../i18n/zh/addByAddress';
import { leaveHubUrl } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';

jest.setTimeout(20000);

const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';

const companyNetwork: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

function props() {
  return {
    navigation: mockNavigation<'AddByAddress'>(),
    route: mockRoute('AddByAddress'),
  } as RootScreenProps<'AddByAddress'>;
}

/** Pre-V10 native package: no `core.discover`; 3389 uses the RSP dial. */
function legacyCore(fixtures: Partial<FakeFixtures> = {}): FakeCore {
  const core = createFakeCore({ network: netCompanyVpn, ...fixtures });
  core.discover = undefined;
  return core;
}

function probeEvent(
  probeId: string,
  port: number,
  outcome: CoreEventOf<'probe.result'>['outcome'],
  ms: number | null = 23,
  hello?: unknown,
): CoreEventOf<'probe.result'> {
  const event: CoreEventOf<'probe.result'> = {
    type: 'probe.result',
    probeId,
    hostId: null,
    path: 'intranet',
    addr: '10.30.12.48',
    port,
    transport: 'tcp',
    outcome,
    ms,
    last: true,
  };
  return hello === undefined
    ? event
    : ({ ...event, hello } as CoreEventOf<'probe.result'>);
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function renderScreen(
  core: FakeCore = createFakeCore(),
  leftoverHubUrl = false,
) {
  const screenProps = props();
  const kv = new MemoryKv();
  if (leftoverHubUrl) {
    leaveHubUrl(kv);
  }
  const view = await renderWithProviders(
    <AddByAddressScreen {...screenProps} />,
    { core, kv, settings: { networks: [companyNetwork] } },
  );
  await advance();
  return { ...view, ...screenProps, core };
}

async function findAddress(host: string, port = '') {
  await fireEvent.changeText(screen.getByLabelText('IP 地址或主机名'), host);
  if (port !== '') {
    await fireEvent.changeText(screen.getByLabelText('Host 端口'), port);
  }
  await fireEvent.press(screen.getByRole('button', { name: '查找' }));
  await advance();
}

async function emits(core: FakeCore, events: CoreEventOf<'probe.result'>[]) {
  for (const event of events) {
    await act(async () => {
      core.emit(event);
      await Promise.resolve();
    });
  }
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('AddByAddress', () => {
  it('shows the company VPN note, probes 7400/3389/22 and reports running', async () => {
    const core = legacyCore();
    await renderScreen(core);

    expect(screen.getByText('已连接 VPN · 本机 10.99.4.27')).toBeOnTheScreen();
    await findAddress('10.30.12.48');

    const probes = core.calls
      .filter(call => call.method === 'diag.probeAddress')
      .map(call => call.args);
    expect(probes).toEqual([
      ['10.30.12.48', 7400],
      ['10.30.12.48', 3389],
      ['10.30.12.48', 22],
    ]);
    expect(screen.getByText('正在查找…')).toBeOnTheScreen();
  });

  it('shows a known address result and saves it as a second address', async () => {
    // The directory is built from paired hosts, whose OS comes from the last
    // ServerHello; there is no tailnet node name.
    const core = legacyCore({
      pairedHosts: pairedHosts.map(host =>
        host.hostId === 'w3k8p2x7m4q9tb6c'
          ? { ...host, os: 'windows', osVersion: 'Windows 11' }
          : host,
      ),
    });
    const { kv, navigation } = await renderScreen(core);
    await findAddress('10.30.12.48');

    await emits(core, [
      probeEvent('p1', 7400, 'ok', 23),
      probeEvent('p2', 3389, 'ok'),
      probeEvent('p3', 22, 'timeout'),
    ]);
    await advance();

    expect(screen.getByText('找到 Host')).toBeOnTheScreen();
    expect(screen.getByText('Windows 11')).toBeOnTheScreen();
    expect(
      screen.getByText(
        '这个地址就是「工位 Windows」登记的地址，会作为它的连接地址保存，不用重新配对。',
      ),
    ).toBeOnTheScreen();
    expect(screen.getByText('开着')).toBeOnTheScreen();
    expect(screen.getByText('没有响应')).toBeOnTheScreen();

    await fireEvent.press(
      screen.getByRole('button', {
        name: '添加为「工位 Windows」的内网地址',
      }),
    );
    await advance();

    // The servers namespace is saved on this phone.
    expect(localSyncGet(kv, 'servers')?.value).toMatchObject({
      entries: [
        {
          kind: 'computer',
          host: '10.30.12.48',
          port: 7400,
          hostId: 'w3k8p2x7m4q9tb6c',
          networkId: 'company',
        },
      ],
    });
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });

  it('switches a known result to a separate new pairing', async () => {
    const core = legacyCore();
    const { navigation } = await renderScreen(core);
    await findAddress('10.30.12.48');
    await emits(core, [
      probeEvent('p1', 7400, 'ok'),
      probeEvent('p2', 3389, 'ok'),
      probeEvent('p3', 22, 'timeout'),
    ]);
    await advance();

    await fireEvent.press(
      screen.getByRole('button', { name: '当作新电脑配对' }),
    );
    expect(
      screen.getByText('会多出一台，和「工位 Windows」分开记。选一种方式：'),
    ).toBeOnTheScreen();
    expect(
      screen.getByRole('button', { name: '还是作为第二个地址添加' }),
    ).toBeOnTheScreen();
    // No dead, permanently disabled code row: only the reason and 去扫码.
    expect(
      screen.queryByRole('button', {
        name: [addByAddress.pair.code, addByAddress.pair.missingHello].join(
          '，',
        ),
      }),
    ).toBeNull();
    expect(
      screen.getByText('当前版本查不到这台电脑的 ID，请用扫码配对'),
    ).toBeOnTheScreen();

    await fireEvent.press(screen.getByRole('button', { name: '去扫码' }));
    expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', {
      tab: 'scan',
    });
  });

  it('shows the new pairing area for a hello with an unknown hostId and saves before navigating', async () => {
    const core = legacyCore();
    const { kv, navigation } = await renderScreen(core);
    await findAddress('10.30.99.9');
    await emits(core, [
      probeEvent('p1', 7400, 'ok', 18, {
        hostId: 'newhost123',
        name: '新电脑',
        os: 'windows',
        hostVersion: '0.3.0',
        fp: 'new-fp',
      }),
      probeEvent('p2', 3389, 'refused'),
      probeEvent('p3', 22, 'timeout'),
    ]);
    await advance();

    expect(
      screen.getByText('这台电脑还没配对。选一种方式：'),
    ).toBeOnTheScreen();
    await fireEvent.press(
      screen.getByRole('button', {
        name: [addByAddress.pair.code, addByAddress.pair.codeSubtitle].join(
          '，',
        ),
      }),
    );
    await advance();

    // The servers namespace is saved on this phone.
    expect(localSyncGet(kv, 'servers')?.value).toMatchObject({
      entries: [
        {
          kind: 'computer',
          host: '10.30.99.9',
          port: 7400,
          hostId: 'newhost123',
          name: '新电脑',
        },
      ],
    });
    // V11 C6: a Host outside the directory pairs by address and code.
    expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', {
      tab: 'code',
      addr: '10.30.99.9',
      port: 7400,
    });
  });

  it.each([
    ['no Hub address', false],
    ['a Hub address left in MMKV', true],
  ] as const)(
    'pairs a directory Host by address and code with %s',
    async (_label, leftoverHubUrl) => {
      const core = legacyCore();
      const { navigation } = await renderScreen(core, leftoverHubUrl);
      await findAddress('10.30.12.48');
      await emits(core, [
        probeEvent('p1', 7400, 'ok', 18, {
          hostId: OFFICE_HOST_ID,
          name: '工位 Windows',
          os: 'windows',
          hostVersion: '0.3.0',
          fp: 'changed-fp',
        }),
        probeEvent('p2', 3389, 'refused'),
        probeEvent('p3', 22, 'timeout'),
      ]);
      await advance();

      await fireEvent.press(
        screen.getByRole('button', {
          name: [addByAddress.pair.code, addByAddress.pair.codeSubtitle].join(
            '，',
          ),
        }),
      );
      await advance();

      expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', {
        tab: 'code',
        addr: '10.30.12.48',
        port: 7400,
      });
    },
  );

  it('pairs a Host outside the directory by address despite a leftover Hub address', async () => {
    const core = legacyCore();
    const { navigation } = await renderScreen(core, true);
    await findAddress('10.30.99.9', '7401');
    await emits(core, [
      probeEvent('p1', 7401, 'ok', 18, {
        hostId: 'newhost123',
        name: '新电脑',
        os: 'windows',
        hostVersion: '0.3.0',
        fp: 'new-fp',
      }),
      probeEvent('p2', 3389, 'refused'),
      probeEvent('p3', 22, 'timeout'),
    ]);
    await advance();

    await fireEvent.press(
      screen.getByRole('button', {
        name: [addByAddress.pair.code, addByAddress.pair.codeSubtitle].join(
          '，',
        ),
      }),
    );
    await advance();

    expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', {
      tab: 'code',
      addr: '10.30.99.9',
      port: 7401,
    });
  });

  it('times out after five seconds and treats missing ports as closed', async () => {
    const core = legacyCore();
    await renderScreen(core);
    await findAddress('10.30.12.48');

    await advance(5000);

    expect(screen.getByText('没有找到 Host')).toBeOnTheScreen();
    expect(screen.getByText('端口 7400 没有响应')).toBeOnTheScreen();
    expect(screen.getByText('未开放')).toBeOnTheScreen();
    expect(screen.getByText('没有响应')).toBeOnTheScreen();
  });

  it('offers system RDP when Host is missing but port 3389 is open', async () => {
    const core = legacyCore();
    const { platform } = await renderScreen(core);
    await findAddress('10.30.12.48');
    platform.calls.splice(0);
    await emits(core, [
      probeEvent('p1', 7400, 'refused'),
      probeEvent('p2', 3389, 'ok'),
      probeEvent('p3', 22, 'timeout'),
    ]);
    await advance();

    await fireEvent.press(
      screen.getByRole('button', { name: '用系统远程桌面打开' }),
    );
    expect(platform.calls[0]).toEqual({
      method: 'canOpenUrl',
      args: ['rdp://full%20address=s:10.30.12.48:3389'],
    });
    expect(platform.calls[1]).toEqual({
      method: 'openUrl',
      args: ['rdp://full%20address=s:10.30.12.48:3389'],
    });
    expect(platform.opened[0]).toBe('rdp://full%20address=s:10.30.12.48:3389');
  });

  it('guides to Windows App and opens its App Store page', async () => {
    const core = legacyCore();
    const { platform } = await renderScreen(core);
    platform.openUrlResult = false;
    await findAddress('10.30.12.48');
    await emits(core, [
      probeEvent('p1', 7400, 'refused'),
      probeEvent('p2', 3389, 'ok'),
      probeEvent('p3', 22, 'timeout'),
    ]);
    await advance();

    await fireEvent.press(
      screen.getByRole('button', { name: '用系统远程桌面打开' }),
    );
    await advance();

    expect(screen.getByText('需要 Windows App')).toBeOnTheScreen();
    expect(
      screen.getByText(
        '微软的 Windows App（原「远程桌面」）能直接打开这台电脑。',
      ),
    ).toBeOnTheScreen();

    platform.openUrlResult = true;
    await fireEvent.press(screen.getByRole('button', { name: '去 App Store' }));
    await advance();

    expect(platform.opened).toContain(
      'itms-apps://apps.apple.com/app/id714464092',
    );
    expect(screen.queryByText('需要 Windows App')).toBeNull();
  });

  it('reports unsupported probeAddress without probing the other ports', async () => {
    const core = legacyCore();
    core.failNext('diag.probeAddress', new RsCoreError('unsupported', 'x'));
    await renderScreen(core);
    await findAddress('10.30.12.48');

    expect(screen.getByText('当前版本还不能按地址查找')).toBeOnTheScreen();
    expect(
      core.calls.filter(call => call.method === 'diag.probeAddress'),
    ).toHaveLength(1);
  });

  it('rejects port zero before calling probeAddress', async () => {
    const core = legacyCore();
    await renderScreen(core);
    await findAddress('10.30.12.48', '0');

    expect(screen.getByText('端口是 1 到 65535 的数字')).toBeOnTheScreen();
    expect(
      core.calls.filter(call => call.method === 'diag.probeAddress'),
    ).toHaveLength(0);
  });
});
