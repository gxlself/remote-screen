import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import AddServerScreen from '.';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import * as fixtures from '../../native/rscore/fixtures';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type {
  CoreEventOf,
  DiscoverOutcome,
  DiscoverProbe,
  SshServer,
} from '../../native/rscore/types';
import { localSyncGet } from '../../features/data/localSync';
import type { ServersValue } from '../../features/servers/manual';
import type { KvStore } from '../../features/storage/kv';
import { MemoryKv } from '../../features/storage/kv';
import { hideToast, ToastHost } from '../../components/ToastHost';
import { failLocalSync, seedLocalSync } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';

const COMPANY = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

const clients: QueryClient[] = [];

function props(serverId?: string) {
  return {
    navigation: mockNavigation<'AddServer'>(),
    route: mockRoute(
      'AddServer',
      serverId === undefined ? undefined : { serverId },
    ),
  } as RootScreenProps<'AddServer'>;
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function mount(options?: {
  serverId?: string;
  core?: FakeCore;
  kv?: KvStore;
  /** 外网访问 (Tailscale); the Tailscale SSH tests below need it on. */
  tailnetEnabled?: boolean;
  platform?: Parameters<typeof renderWithProviders>[1] extends infer O
    ? O extends { platform?: infer P }
      ? P
      : never
    : never;
}) {
  const screenProps = props(options?.serverId);
  const view = await renderWithProviders(
    <>
      <AddServerScreen {...screenProps} />
      <ToastHost />
    </>,
    {
      core: options?.core,
    kv: options?.kv,
      platform: options?.platform,
      settings: {
        networks: [COMPANY],
        'tailnet.enabled': options?.tailnetEnabled ?? true,
      },
    },
  );
  clients.push(view.queryClient);
  await act(async () => {
    await Promise.resolve();
  });
  return { ...view, ...screenProps };
}

function probeEvent(
  probeId = 'p1',
  patch: Partial<CoreEventOf<'probe.result'>> = {},
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId,
    hostId: null,
    path: 'manual',
    addr: '10.30.0.5',
    port: 22,
    transport: 'tcp',
    outcome: 'ok',
    ms: 31,
    last: true,
    ...patch,
  };
}

/** Pre-V10 native package: no `core.discover`; the test dials RSP. */
function legacyCore(): FakeCore {
  const core = createFakeCore();
  core.discover = undefined;
  return core;
}

function tcpOutcome(core: FakeCore, outcome: DiscoverOutcome, ms = 4) {
  return jest
    .spyOn(core.discover!, 'probe')
    .mockImplementation(async (addr, port) => ({ addr, port, outcome, ms }));
}

async function fillTarget() {
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
}

async function pickJump(address: string) {
  await fireEvent.press(screen.getByLabelText('经跳板机连接'));
  await fireEvent.press(screen.getByLabelText('选择跳板机'));
  await fireEvent.changeText(screen.getByLabelText('其他地址'), address);
  await fireEvent.press(screen.getByRole('button', { name: '用这个' }));
}

async function pressTest() {
  await fireEvent.press(screen.getByRole('button', { name: '测试连接' }));
}

/** The servers namespace saved on this phone, if anything was saved. */
function manualValue(kv: KvStore) {
  return localSyncGet<ServersValue>(kv, 'servers')?.value;
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await act(async () => hideToast());
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('requires the address and username before saving', async () => {
  const { kv, navigation } = await mount();
  await fireEvent.press(await screen.findByRole('button', { name: '保存' }));
  expect(screen.getAllByText('必填')).toHaveLength(2);
  expect(manualValue(kv)).toBeUndefined();
  expect(navigation.goBack).not.toHaveBeenCalled();
});

it('suggests a jump host and chooses the company network', async () => {
  await mount();
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  expect(
    screen.getByText('10.30.20.31 在公司内网，一般要经跳板机'),
  ).toBeOnTheScreen();
  expect(screen.getByText('公司网络')).toBeOnTheScreen();
});

it('offers no Tailscale SSH while Tailscale is switched off', async () => {
  await mount({ tailnetEnabled: false });
  expect(screen.queryByLabelText('Tailscale SSH')).toBeNull();
  expect(screen.queryAllByText(/Tailscale/)).toHaveLength(0);
});

it('switches Tailscale SSH guidance with tailnet address', async () => {
  await mount();
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  await fireEvent.press(screen.getByLabelText('Tailscale SSH'));
  expect(
    screen.getByText(
      '这台机器不在 Tailscale 网络里，不能用 Tailscale SSH。换成密钥或密码登录就行。',
    ),
  ).toBeOnTheScreen();
  expect(
    screen.getByText('先换一种登录方式，再点「测试连接」'),
  ).toBeOnTheScreen();
  await fireEvent.changeText(screen.getByLabelText('地址'), '100.64.0.31');
  expect(
    screen.getByText('用 Tailscale 身份登录，不用密钥和密码'),
  ).toBeOnTheScreen();
});

it('copies the device key and resets the label after two seconds', async () => {
  const { platform } = await mount();
  await fireEvent.press(screen.getByLabelText('密钥'));
  expect(await screen.findByText('rs-iphone-se')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '复制公钥' }));
  expect(platform.copied[0]).toMatch(/^ecdsa-sha2-nistp256 /);
  expect(screen.getByRole('button', { name: '已复制' })).toBeOnTheScreen();
  await advance(2000);
  expect(screen.getByRole('button', { name: '复制公钥' })).toBeOnTheScreen();
});

it('does not render a password input', async () => {
  await mount();
  await fireEvent.press(screen.getByLabelText('密码'));
  expect(
    screen.getByText(
      '连接时在终端里输入密码。想免密登录，连上后到「SSH 密钥」把公钥装上。',
    ),
  ).toBeOnTheScreen();
  for (const label of ['名称', '用户名', '地址', '端口']) {
    expect(screen.getByLabelText(label)).toHaveProp('secureTextEntry', false);
  }
});

it('accepts a free-form jump address', async () => {
  await mount();
  await fireEvent.press(screen.getByLabelText('经跳板机连接'));
  await fireEvent.press(screen.getByLabelText('选择跳板机'));
  await fireEvent.changeText(screen.getByLabelText('其他地址'), '10.30.0.5:22');
  await fireEvent.press(screen.getByRole('button', { name: '用这个' }));
  expect(
    screen.getByLabelText('选择跳板机，当前 10.30.0.5:22'),
  ).toBeOnTheScreen();
});

it('tests the jump host and reports success with its latency', async () => {
  const { core } = await mount({ core: legacyCore() });
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  await fireEvent.press(screen.getByLabelText('经跳板机连接'));
  await fireEvent.press(screen.getByLabelText('选择跳板机'));
  await fireEvent.changeText(screen.getByLabelText('其他地址'), '10.30.0.5:22');
  await fireEvent.press(screen.getByRole('button', { name: '用这个' }));
  await fireEvent.press(screen.getByRole('button', { name: '测试连接' }));
  expect(core.calls).toContainEqual({
    method: 'diag.probeAddress',
    args: ['10.30.0.5', 22],
  });
  expect(screen.getByText('正在测试…')).toBeOnTheScreen();
  await act(async () => core.emit(probeEvent('p1')));
  expect(screen.getByText('经 10.30.0.5:22 端口能连上 · 31 ms')).toBeOnTheScreen();
});

it('times out when no probe result arrives', async () => {
  await mount({ core: legacyCore() });
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  await fireEvent.press(screen.getByRole('button', { name: '测试连接' }));
  await advance(5000);
  expect(screen.getByText('没有收到结果，稍后再试')).toBeOnTheScreen();
});

it('shows a generic failure for other probe errors', async () => {
  const core = legacyCore();
  core.failNext('diag.probeAddress', new RsCoreError('network', 'x'));
  await mount({ core });
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  await fireEvent.press(screen.getByRole('button', { name: '测试连接' }));
  await advance();
  expect(screen.getByText('连接失败')).toBeOnTheScreen();
});

it('shows the unsupported message when probing is unavailable', async () => {
  const core = legacyCore();
  core.failNext('diag.probeAddress', new RsCoreError('unsupported', 'x'));
  await mount({ core });
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  await fireEvent.press(screen.getByRole('button', { name: '测试连接' }));
  await advance();
  expect(
    screen.getByText('当前版本还不能预先测试，保存后直接连接试试'),
  ).toBeOnTheScreen();
});

it('applies a probe result that arrives before probeAddress returns', async () => {
  const core = legacyCore();
  const probeAddress = jest
    .spyOn(core.diag, 'probeAddress')
    .mockImplementation(async () => {
      // The final result lands before the probe id is handed back.
      core.emit(probeEvent('p1'));
      return 'p1';
    });
  await mount({ core });
  await fillTarget();
  await pressTest();
  await advance();
  expect(probeAddress).toHaveBeenCalledWith('10.30.20.31', 22);
  expect(screen.getByText('端口能连上 · 31 ms')).toBeOnTheScreen();
  await advance(5000);
  expect(screen.getByText('端口能连上 · 31 ms')).toBeOnTheScreen();
  expect(screen.queryByText('没有收到结果，稍后再试')).toBeNull();
});

it('keeps waiting when an early result belongs to another probe', async () => {
  const core = legacyCore();
  jest.spyOn(core.diag, 'probeAddress').mockImplementation(async () => {
    core.emit(probeEvent('p0', { outcome: 'refused' }));
    return 'p1';
  });
  await mount({ core });
  await fillTarget();
  await pressTest();
  await advance();
  expect(screen.getByText('正在测试…')).toBeOnTheScreen();
  await act(async () => core.emit(probeEvent('p1')));
  expect(screen.getByText('端口能连上 · 31 ms')).toBeOnTheScreen();
});

describe('with the V10 discover binding', () => {
  it('tests the SSH port with a plain TCP connect', async () => {
    const core = createFakeCore();
    const probe = tcpOutcome(core, 'open');
    await mount({ core });
    await fillTarget();
    await pressTest();
    await advance();
    expect(probe).toHaveBeenCalledTimes(1);
    expect(probe).toHaveBeenCalledWith('10.30.20.31', 22, 'tcp', 3000);
    expect(core.calls.some(call => call.method === 'diag.probeAddress')).toBe(
      false,
    );
    expect(screen.getByText('端口能连上 · 4 ms')).toBeOnTheScreen();
  });

  it.each([
    ['refused', '端口没有开放'],
    ['timeout', '3 秒内没有响应'],
  ] as const)('reports a %s SSH port', async (outcome, reason) => {
    const core = createFakeCore();
    tcpOutcome(core, outcome);
    await mount({ core });
    await fillTarget();
    await pressTest();
    await advance();
    expect(screen.getByText('连不上')).toBeOnTheScreen();
    expect(screen.getByText(reason)).toBeOnTheScreen();
  });

  it('probes the jump host when one is enabled', async () => {
    const core = createFakeCore();
    const probe = tcpOutcome(core, 'open', 9);
    await mount({ core });
    await fillTarget();
    await pickJump('10.30.0.5:2200');
    await pressTest();
    await advance();
    expect(probe).toHaveBeenCalledWith('10.30.0.5', 2200, 'tcp', 3000);
    expect(screen.getByText('经 10.30.0.5:2200 端口能连上 · 9 ms')).toBeOnTheScreen();
  });

  it('drops a result that arrives after the test was cancelled', async () => {
    const core = createFakeCore();
    let resolve: (result: DiscoverProbe) => void = () => {};
    jest.spyOn(core.discover!, 'probe').mockImplementation(
      () =>
        new Promise(done => {
          resolve = done;
        }),
    );
    await mount({ core });
    await fillTarget();
    await pressTest();
    expect(screen.getByText('正在测试…')).toBeOnTheScreen();
    await fireEvent.changeText(screen.getByLabelText('名称'), 'build-srv');
    await act(async () =>
      resolve({ addr: '10.30.20.31', port: 22, outcome: 'open', ms: 4 }),
    );
    await advance();
    expect(screen.getByText('还没测试')).toBeOnTheScreen();
    expect(screen.queryByText('端口能连上 · 4 ms')).toBeNull();
  });

  it('shows a generic failure when the TCP probe throws', async () => {
    const core = createFakeCore();
    jest
      .spyOn(core.discover!, 'probe')
      .mockRejectedValue(new RsCoreError('network', 'x'));
    await mount({ core });
    await fillTarget();
    await pressTest();
    await advance();
    expect(screen.getByText('连接失败')).toBeOnTheScreen();
  });
});

it('saves the server on this phone', async () => {
  const { kv, navigation } = await mount();
  await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
  await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
  await fireEvent.press(screen.getByRole('button', { name: '保存' }));
  await advance();
  const value = manualValue(kv);
  expect(value?.entries[0]).toMatchObject({
    kind: 'ssh',
    name: '10.30.20.31',
    user: 'alice',
    jump: null,
    networkId: 'company',
    auth: 'deviceKey',
  });
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('edits and deletes a manual server', async () => {
  const server: SshServer = {
    ...(fixtures.sshServers.find(
      candidate => candidate.serverId === 'manual-build-srv',
    ) as SshServer),
  };
  const core = createFakeCore({ sshServers: [server] });
  const kv = new MemoryKv();
  seedLocalSync(kv, 'servers', {
    version: 1,
    value: {
      v: 1,
      entries: [
        {
          id: 'manual-build-srv',
          kind: 'ssh',
          name: 'build-srv',
          host: '10.30.20.31',
          port: 22,
          user: 'alice',
          auth: 'password',
          jump: 'jump-01',
          networkId: 'company',
          addedAtUs: FIXTURE_NOW,
          updatedAtUs: FIXTURE_NOW,
        },
      ],
      tailscaleSsh: {},
    },
  });
  const { navigation } = await mount({
    serverId: 'manual-build-srv',
    core,
    kv,
  });
  expect(await screen.findByText('编辑服务器')).toBeOnTheScreen();
  expect(screen.getByLabelText('名称')).toHaveProp('value', 'build-srv');
  await fireEvent.press(screen.getByRole('button', { name: '删除服务器' }));
  expect(screen.getByText('删除 build-srv？')).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '删除' }));
  await advance();
  expect(manualValue(kv)?.entries).toEqual([]);
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('shows a read-only directory server without a save action', async () => {
  const directoryServer = fixtures.sshServers.find(
    candidate => candidate.name === 'prod-api-01',
  );
  expect(directoryServer).toBeDefined();
  await mount({ serverId: directoryServer!.serverId });
  expect(
    await screen.findByText('这台服务器是自动发现的，不能在这里改'),
  ).toBeOnTheScreen();
  expect(screen.queryByRole('button', { name: '保存' })).toBeNull();
});

function seedBuildSrv(kv: KvStore) {
  seedLocalSync(kv, 'servers', {
    version: 1,
    value: {
      v: 1,
      entries: [
        {
          id: 'manual-build-srv',
          kind: 'ssh',
          name: 'build-srv',
          host: '10.30.20.31',
          port: 22,
          user: 'alice',
          auth: 'password',
          jump: null,
          networkId: 'company',
          addedAtUs: FIXTURE_NOW,
          updatedAtUs: FIXTURE_NOW,
        },
      ],
      tailscaleSsh: {},
    },
  });
}

/** The `beforeRemove` listener the screen registered last. */
function beforeRemove(navigation: RootScreenProps<'AddServer'>['navigation']) {
  const addListener = navigation.addListener as unknown as jest.Mock;
  const calls = addListener.mock.calls.filter(
    ([name]) => name === 'beforeRemove',
  );
  return calls[calls.length - 1]?.[1] as (event: {
    preventDefault: () => void;
    data: { action: { type: string } };
  }) => void;
}

describe('save feedback', () => {
  it('names the first field to fix next to the save button', async () => {
    const { kv } = await mount();
    await fireEvent.changeText(screen.getByLabelText('地址'), '10.30.20.31');
    await fireEvent.press(screen.getByTestId('add-server-save'));
    expect(screen.getByTestId('add-server-fix-above')).toHaveTextContent(
      '「用户名」还要改一下',
    );
    await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
    expect(screen.queryByTestId('add-server-fix-above')).toBeNull();
    expect(manualValue(kv)).toBeUndefined();
  });

  it('chains the return key from name to port', async () => {
    await mount();
    expect(screen.getByLabelText('名称')).toHaveProp('returnKeyType', 'next');
    expect(screen.getByLabelText('用户名')).toHaveProp('returnKeyType', 'next');
    expect(screen.getByLabelText('地址')).toHaveProp('returnKeyType', 'next');
    expect(screen.getByLabelText('地址')).toHaveProp('keyboardType', 'url');
    expect(screen.getByLabelText('端口')).toHaveProp('returnKeyType', 'done');
    expect(screen.getByLabelText('端口')).toHaveProp('maxLength', 5);
  });

  it('shows why Tailscale SSH cannot be saved for a LAN address', async () => {
    const { kv } = await mount();
    await fillTarget();
    await fireEvent.press(screen.getByLabelText('Tailscale SSH'));
    await fireEvent.press(screen.getByTestId('add-server-save'));
    expect(screen.getByTestId('add-server-fix-above')).toHaveTextContent(
      '「登录方式」还要改一下',
    );
    expect(manualValue(kv)).toBeUndefined();
  });

  it('asks for a key before saving key login on a phone without one', async () => {
    const core = createFakeCore({ sshKeys: [] });
    const { kv } = await mount({ core });
    expect(await screen.findByTestId('auth-key-empty')).toBeOnTheScreen();
    await fillTarget();
    await fireEvent.press(screen.getByTestId('add-server-save'));
    expect(screen.getByTestId('auth-error')).toHaveTextContent(
      '这台手机还没有 SSH 密钥。先去生成一把，或者改用密码登录。',
    );
    expect(manualValue(kv)).toBeUndefined();
    await fireEvent.press(screen.getByLabelText('密码'));
    await fireEvent.press(screen.getByTestId('add-server-save'));
    await advance();
    expect(manualValue(kv)?.entries).toHaveLength(1);
  });

  it('confirms a new server with a toast', async () => {
    const { navigation } = await mount();
    await fillTarget();
    await fireEvent.press(screen.getByTestId('add-server-save'));
    await advance();
    expect(screen.getByTestId('global-toast')).toHaveProp(
      'accessibilityLabel',
      '已添加',
    );
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });

  it('keeps the save failure free of raw error text', async () => {
    const { kv, navigation } = await mount();
    failLocalSync(kv, 'servers', new Error('EACCES: disk'));
    await fillTarget();
    await fireEvent.press(screen.getByTestId('add-server-save'));
    await advance();
    expect(screen.getByTestId('save-error')).toHaveTextContent(
      '没保存上，稍后再试',
    );
    expect(navigation.goBack).not.toHaveBeenCalled();
  });
});

describe('leaving with changes', () => {
  it('asks before discarding edits and dispatches the original action', async () => {
    const { navigation } = await mount();
    const pristine = {
      preventDefault: jest.fn(),
      data: { action: { type: 'GO_BACK' } },
    };
    await act(async () => beforeRemove(navigation)(pristine));
    expect(pristine.preventDefault).not.toHaveBeenCalled();

    await fireEvent.changeText(screen.getByLabelText('用户名'), 'alice');
    const event = {
      preventDefault: jest.fn(),
      data: { action: { type: 'GO_BACK' } },
    };
    await act(async () => beforeRemove(navigation)(event));
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(screen.getByText('放弃修改？')).toBeOnTheScreen();

    await fireEvent.press(screen.getByRole('button', { name: '继续编辑' }));
    expect(screen.queryByText('放弃修改？')).toBeNull();
    expect(navigation.dispatch).not.toHaveBeenCalled();

    await act(async () => beforeRemove(navigation)(event));
    await fireEvent.press(screen.getByTestId('discard-confirm'));
    expect(navigation.dispatch).toHaveBeenCalledWith({ type: 'GO_BACK' });
  });

  it('does not ask after a successful save', async () => {
    const { navigation } = await mount();
    await fillTarget();
    await fireEvent.press(screen.getByTestId('add-server-save'));
    await advance();
    const event = {
      preventDefault: jest.fn(),
      data: { action: { type: 'GO_BACK' } },
    };
    await act(async () => beforeRemove(navigation)(event));
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it('uses a pinned back chevron instead of a close button', async () => {
    const { navigation } = await mount();
    expect(screen.queryByLabelText('关闭')).toBeNull();
    await fireEvent.press(screen.getByLabelText('返回'));
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });
});

describe('editing and deleting', () => {
  it('shows a spinner and a way back while the server loads', async () => {
    const core = createFakeCore();
    jest
      .spyOn(core.ssh, 'servers')
      .mockImplementation(() => new Promise(() => {}));
    const { navigation } = await mount({ serverId: 'manual-x', core });
    expect(screen.getByTestId('add-server-loading')).toBeOnTheScreen();
    await fireEvent.press(screen.getByLabelText('返回'));
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });

  it('keeps the delete dialog busy and shows a failure under the row', async () => {
    const kv = new MemoryKv();
    seedBuildSrv(kv);
    const { navigation } = await mount({ serverId: 'manual-build-srv', kv });
    expect(await screen.findByText('编辑服务器')).toBeOnTheScreen();
    failLocalSync(kv, 'servers', new Error('offline'));
    await fireEvent.press(screen.getByRole('button', { name: '删除服务器' }));
    await fireEvent.press(screen.getByTestId('delete-server-confirm'));
    await advance();
    expect(screen.queryByText('删除 build-srv？')).toBeNull();
    expect(screen.getByTestId('delete-error')).toHaveTextContent(
      '没删掉，稍后再试',
    );
    expect(screen.queryByTestId('save-error')).toBeNull();
    expect(navigation.goBack).not.toHaveBeenCalled();
  });

  it('says so when copying the public key fails', async () => {
    const { platform } = await mount();
    jest.spyOn(platform, 'copyText').mockRejectedValue(new Error('denied'));
    expect(await screen.findByText('rs-iphone-se')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '复制公钥' }));
    expect(screen.getByTestId('auth-key-copy-failed')).toHaveTextContent(
      '没复制上，再试一次',
    );
    expect(screen.getByRole('button', { name: '复制公钥' })).toBeOnTheScreen();
  });
});

describe('jump host picker', () => {
  it('explains an empty or invalid jump address instead of ignoring it', async () => {
    await mount();
    await fireEvent.press(screen.getByLabelText('经跳板机连接'));
    await fireEvent.press(screen.getByLabelText('选择跳板机'));
    await fireEvent.press(screen.getByRole('button', { name: '用这个' }));
    expect(screen.getByText('先填一个地址')).toBeOnTheScreen();
    await fireEvent.changeText(screen.getByLabelText('其他地址'), 'a b');
    await fireEvent.press(screen.getByRole('button', { name: '用这个' }));
    expect(screen.getByText('不像是 IP 或主机名')).toBeOnTheScreen();
    expect(screen.getByTestId('jump-picker')).toBeOnTheScreen();
  });

  it('says there is nothing to pick when no other server exists', async () => {
    const core = createFakeCore({ sshServers: [] });
    await mount({ core });
    await fireEvent.press(screen.getByLabelText('经跳板机连接'));
    await fireEvent.press(screen.getByLabelText('选择跳板机'));
    expect(screen.getByTestId('jump-no-servers')).toHaveTextContent(
      '还没有别的服务器可选，在下面填跳板机的地址。',
    );
  });

  it('shows the real problem with a jump address on save', async () => {
    await mount();
    await fillTarget();
    await fireEvent.press(screen.getByLabelText('经跳板机连接'));
    await fireEvent.press(screen.getByTestId('add-server-save'));
    expect(screen.getByTestId('jump-error')).toHaveTextContent('必填');
  });

  it('marks the current network with a check', async () => {
    await mount();
    await fireEvent.press(
      await screen.findByLabelText('需要的网络：不需要，点按更改'),
    );
    expect(screen.getByTestId('network-option-none')).toContainElement(
      screen.getByTestId('picker-selected'),
    );
  });
});
