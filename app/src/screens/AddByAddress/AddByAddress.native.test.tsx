import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
} from '@testing-library/react-native';
import AddByAddressScreen from '.';
import type { RootScreenProps } from '../../app/navigation/routes';
import { installFakeAdapters } from '../../features/adapters/testing/fakeAdapters';
import type { NetworkEntry } from '../../features/settings/schema';
import { addByAddress } from '../../i18n/zh/addByAddress';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { netCompanyVpn } from '../../native/rscore/fixtures';
import type { CoreEventOf, DiscoverOutcome } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { localSyncGet } from '../../features/data/localSync';
import type { KvStore } from '../../features/storage/kv';
import type { ServersValue } from '../../features/servers/manual';

jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    navigate: jest.fn(),
    dispatch: jest.fn(),
    isReady: jest.fn(() => true),
  },
}));
// Built-in adapters include 'rdp'. Model that build so Remote
// Desktop opens the AdapterSession instead of the external Windows App.
jest.mock('../../features/adapters/logic', () => ({
  ...jest.requireActual('../../features/adapters/logic'),
  BUILTIN_PROTOCOLS: ['vnc', 'rdp'],
}));

const navigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as { navigate: jest.Mock; dispatch: jest.Mock };

/** The session replaces this page instead of stacking on top of it. */
function replacedWith(name: string, params: unknown) {
  return { type: 'REPLACE', payload: { name, params } };
}

jest.setTimeout(20000);

const companyNetwork: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

function probeEvent(
  probeId: string,
  port: number,
  outcome: CoreEventOf<'probe.result'>['outcome'],
  hostId: string | null = null,
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId,
    hostId,
    path: 'intranet',
    addr: '10.30.99.9',
    port,
    transport: 'tcp',
    outcome,
    ms: 21,
    last: true,
  };
}

function nativeOutcomes(
  core: FakeCore,
  outcomes: Partial<Record<number, DiscoverOutcome>>,
) {
  return jest
    .spyOn(core.discover!, 'probe')
    .mockImplementation(async (addr, port) => ({
      addr,
      port,
      outcome: outcomes[port] ?? 'timeout',
      ms: 4,
    }));
}

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function renderScreen(core: FakeCore) {
  const navigation = mockNavigation<'AddByAddress'>();
  const view = await renderWithProviders(
    <AddByAddressScreen
      navigation={navigation}
      route={mockRoute('AddByAddress')}
    />,
    { core, settings: { networks: [companyNetwork] } },
  );
  await advance();
  return {
    ...view,
    navigation: navigation as RootScreenProps<'AddByAddress'>['navigation'] & {
      [k: string]: jest.Mock;
    },
  };
}

async function findAddress(host: string) {
  await fireEvent.changeText(screen.getByLabelText('IP 地址或主机名'), host);
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
  await advance();
}

/** The servers namespace saved on this phone, if anything was saved. */
function savedServers(kv: KvStore) {
  return localSyncGet<ServersValue>(kv, 'servers')?.value;
}

beforeEach(() => {
  jest.useFakeTimers();
  navigationRef.navigate.mockReset();
  navigationRef.dispatch.mockReset();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('probes 3389, 5900 and 22 through discover and dials RSP only for 7400', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  await renderScreen(core);
  await findAddress('10.30.99.9');

  expect(
    core.calls
      .filter(call => call.method === 'diag.probeAddress')
      .map(call => call.args),
  ).toEqual([['10.30.99.9', 7400]]);
  expect(
    core.calls
      .filter(call => call.method === 'discover.probe')
      .map(call => call.args),
  ).toEqual([
    ['10.30.99.9', 3389, 'rdp', 3000],
    ['10.30.99.9', 5900, 'vnc', 3000],
    ['10.30.99.9', 22, 'tcp', 3000],
  ]);
});

it.each([
  ['open', addByAddress.actions.addSsh],
  ['refused', addByAddress.services.noResponse],
  ['timeout', addByAddress.services.noResponse],
] as const)(
  'shows the SSH row for TCP outcome %s',
  async (outcome, value) => {
    const core = createFakeCore({ network: netCompanyVpn });
    nativeOutcomes(core, { 22: outcome });
    await renderScreen(core);
    await findAddress('10.30.99.9');
    await emits(core, [probeEvent('p1', 7400, 'refused')]);

    expect(
      screen.getByLabelText([addByAddress.services.ssh, value].join('，')),
    ).toBeOnTheScreen();
  },
);

const SSH_PREFILL = {
  prefill: {
    name: '10.30.99.9',
    host: '10.30.99.9',
    port: 22,
    connectAfterSave: true,
  },
};
const SSH_ROW = [addByAddress.services.ssh, addByAddress.actions.addSsh].join(
  '，',
);

it('offers a primary SSH button when only 22 answers', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  nativeOutcomes(core, { 22: 'open', 3389: 'refused', 5900: 'refused' });
  const { kv, navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [probeEvent('p1', 7400, 'refused')]);

  expect(screen.queryByText(addByAddress.native.connectVnc)).toBeNull();
  expect(screen.queryByText(addByAddress.native.connectRdp)).toBeNull();
  const primary = screen.getByRole('button', {
    name: addByAddress.actions.addSsh,
  });
  expect(primary).toBe(screen.getByTestId('add-address-ssh'));

  await fireEvent.press(primary);

  expect(navigation.replace).toHaveBeenCalledWith('AddServer', SSH_PREFILL);
  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(savedServers(kv)).toBeUndefined();
});

it('keeps Remote Desktop as the primary button and makes the SSH row tappable', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  installFakeAdapters(core);
  nativeOutcomes(core, { 22: 'open', 3389: 'open', 5900: 'refused' });
  const { navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [probeEvent('p1', 7400, 'refused')]);

  expect(
    screen.getByRole('button', { name: addByAddress.native.connectRdp }),
  ).toBeOnTheScreen();
  expect(screen.queryByTestId('add-address-ssh')).toBeNull();
  expect(
    screen.queryByRole('button', { name: addByAddress.actions.addSsh }),
  ).toBeNull();

  const row = screen.getByRole('button', { name: SSH_ROW });
  expect(row).toBe(screen.getByTestId('add-address-ssh-row'));
  expect(
    within(row).getByTestId('list-row-chevron', {
      includeHiddenElements: true,
    }),
  ).toBeOnTheScreen();
  await fireEvent.press(row);

  expect(navigation.replace).toHaveBeenCalledWith('AddServer', SSH_PREFILL);
});

it('names the SSH server after the Host the probe found', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  nativeOutcomes(core, { 22: 'open' });
  const { navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    {
      ...probeEvent('p1', 7400, 'ok', 'newhost123'),
      hello: {
        hostId: 'newhost123',
        name: 'build-mac',
        os: 'macos',
        hostVersion: '0.3.0',
        fp: 'new-fp',
      },
    } as CoreEventOf<'probe.result'>,
  ]);

  expect(screen.getByText('找到 Host 0.3.0')).toBeOnTheScreen();
  // A Remote Host answered, so pairing stays the main path.
  expect(screen.queryByTestId('add-address-ssh')).toBeNull();

  await fireEvent.press(screen.getByRole('button', { name: SSH_ROW }));

  expect(navigation.replace).toHaveBeenCalledWith('AddServer', {
    prefill: {
      name: 'build-mac',
      host: '10.30.99.9',
      port: 22,
      connectAfterSave: true,
    },
  });
});

it('leaves the SSH row inert and offers no SSH button when 22 does not answer', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  nativeOutcomes(core, { 22: 'refused', 3389: 'refused', 5900: 'refused' });
  const { navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [probeEvent('p1', 7400, 'refused')]);

  const row = screen.getByLabelText(
    [addByAddress.services.ssh, addByAddress.services.noResponse].join('，'),
  );
  expect(row).not.toHaveProp('accessibilityRole', 'button');
  expect(screen.queryByTestId('add-address-ssh-row')).toBeNull();
  expect(screen.queryByTestId('add-address-ssh')).toBeNull();
  expect(screen.queryByText(addByAddress.actions.addSsh)).toBeNull();
  expect(navigation.replace).not.toHaveBeenCalled();
});

it('connects with Remote Desktop when 3389 is open', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  installFakeAdapters(core);
  nativeOutcomes(core, { 3389: 'open', 5900: 'refused' });
  const { kv } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    probeEvent('p1', 7400, 'refused'),
    probeEvent('p2', 22, 'timeout'),
  ]);

  // No Host, but Remote Desktop answers: that is the main path, not a miss.
  expect(screen.getByText('找到电脑 · 可以用远程桌面')).toBeOnTheScreen();
  expect(
    screen.getByText('这台电脑没装 Host，不装也能直接连。'),
  ).toBeOnTheScreen();
  expect(screen.queryByText('没有找到 Host')).toBeNull();
  expect(screen.queryByText(addByAddress.services.openRdp)).toBeNull();
  expect(screen.queryByText(addByAddress.native.connectVnc)).toBeNull();
  expect(screen.queryByText(addByAddress.native.off('macos'))).toBeNull();

  await fireEvent.press(
    screen.getByRole('button', { name: addByAddress.native.connectRdp }),
  );
  await advance();
  await advance();

  expect(savedServers(kv)).toMatchObject({
    entries: [
      {
        kind: 'computer',
        host: '10.30.99.9',
        port: 3389,
        hostId: null,
        networkId: 'company',
        os: 'windows',
        protocol: 'rdp',
      },
    ],
  });
  expect(navigationRef.dispatch).toHaveBeenCalledWith(
    replacedWith('AdapterSession', {
      target: { protocol: 'rdp', addr: '10.30.99.9' },
      name: '10.30.99.9',
      addrKind: 'manual',
      context: 'native',
    }),
  );
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

it('puts Screen Sharing first when 5900 and 3389 are both open', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  installFakeAdapters(core);
  nativeOutcomes(core, { 3389: 'open', 5900: 'open' });
  const { kv } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    probeEvent('p1', 7400, 'timeout'),
    probeEvent('p2', 22, 'timeout'),
  ]);

  const buttons = screen
    .getAllByRole('button')
    .map(button => button.props.accessibilityLabel);
  expect(buttons.indexOf(addByAddress.native.connectVnc)).toBeLessThan(
    buttons.indexOf(addByAddress.native.connectRdp),
  );
  expect(screen.getByText(addByAddress.services.vnc)).toBeOnTheScreen();

  await fireEvent.press(
    screen.getByRole('button', { name: addByAddress.native.connectVnc }),
  );
  await advance();
  await advance();

  expect(savedServers(kv)).toMatchObject({
    entries: [{ port: 5900, os: 'macos', protocol: 'vnc', hostId: null }],
  });
  expect(screen.getByText('找到电脑 · 可以用屏幕共享')).toBeOnTheScreen();
  expect(navigationRef.dispatch).toHaveBeenCalledWith(
    replacedWith(
      'AdapterSession',
      expect.objectContaining({
        target: { protocol: 'vnc', addr: '10.30.99.9' },
      }),
    ),
  );
});

it('explains a refused 5900 and links to the sharing guide', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  nativeOutcomes(core, { 5900: 'refused', 3389: 'timeout' });
  const { navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    probeEvent('p1', 7400, 'refused'),
    probeEvent('p2', 22, 'timeout'),
  ]);

  expect(screen.getByText('电脑在线，但没打开屏幕共享')).toBeOnTheScreen();
  expect(screen.queryByText('电脑在线，但没打开远程桌面')).toBeNull();
  expect(screen.queryByText(addByAddress.native.connectVnc)).toBeNull();
  expect(screen.queryByText(addByAddress.native.connectRdp)).toBeNull();

  await fireEvent.press(
    screen.getByRole('button', {
      name: '电脑在线，但没打开屏幕共享，怎么打开',
    }),
  );
  expect(navigation.navigate).toHaveBeenCalledWith('EnableSharing', {
    os: 'macos',
  });
});

it('keeps the old RSP probe without discover', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  core.discover = undefined;
  await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    probeEvent('p1', 7400, 'refused'),
    probeEvent('p2', 3389, 'ok'),
    probeEvent('p3', 22, 'timeout'),
  ]);

  expect(
    core.calls
      .filter(call => call.method === 'diag.probeAddress')
      .map(call => call.args[1]),
  ).toEqual([7400, 3389, 22]);
  expect(core.calls.some(call => call.method.startsWith('discover.'))).toBe(
    false,
  );
  expect(screen.queryByText(addByAddress.services.vnc)).toBeNull();
  expect(screen.queryByText(addByAddress.native.connectRdp)).toBeNull();
  expect(
    screen.getByRole('button', { name: addByAddress.services.openRdp }),
  ).toBeOnTheScreen();
});

it('treats a probe.result that only carries hostId as a Remote Host', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  const { kv, navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    probeEvent('p1', 7400, 'ok', 'newhost123'),
    probeEvent('p2', 22, 'timeout'),
  ]);

  expect(screen.getByText('找到 Host')).toBeOnTheScreen();
  expect(screen.getByText('这台电脑还没配对。选一种方式：')).toBeOnTheScreen();
  expect(screen.queryByText(addByAddress.pair.missingHello)).toBeNull();

  await fireEvent.press(
    screen.getByRole('button', {
      name: [addByAddress.pair.code, addByAddress.pair.codeSubtitle].join('，'),
    }),
  );
  await advance();

  expect(savedServers(kv)).toMatchObject({
    entries: [
      {
        kind: 'computer',
        host: '10.30.99.9',
        port: 7400,
        hostId: 'newhost123',
        name: null,
      },
    ],
  });
  expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', {
    tab: 'code',
    addr: '10.30.99.9',
    port: 7400,
  });
});

it('clears the old result as soon as the address is edited', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  installFakeAdapters(core);
  nativeOutcomes(core, { 3389: 'open' });
  await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [probeEvent('p1', 7400, 'refused')]);
  expect(screen.getByTestId('probe-result-card')).toBeOnTheScreen();
  expect(screen.getByTestId('add-address-connect-rdp')).toBeOnTheScreen();

  await fireEvent.changeText(
    screen.getByLabelText('IP 地址或主机名'),
    '10.30.99.10',
  );

  expect(screen.queryByTestId('probe-result-card')).toBeNull();
  expect(screen.queryByTestId('add-address-connect-rdp')).toBeNull();
});

it('keeps the connect button busy through the whole save and connect', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  installFakeAdapters(core);
  nativeOutcomes(core, { 3389: 'open' });
  const { queryClient } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [probeEvent('p1', 7400, 'refused')]);
  // The save never settles, so the session has not opened yet.
  jest
    .spyOn(queryClient, 'invalidateQueries')
    .mockReturnValue(new Promise(() => {}));

  await fireEvent.press(screen.getByTestId('add-address-connect-rdp'));
  await advance();

  expect(screen.getByTestId('add-address-connect-rdp')).toBeBusy();
  expect(screen.getByTestId('add-address-connect-rdp')).toBeDisabled();
  expect(navigationRef.dispatch).not.toHaveBeenCalled();
});

it('can still save a computer that answers nothing yet', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  nativeOutcomes(core, {});
  const { kv, navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [probeEvent('p1', 7400, 'timeout')]);

  expect(screen.getByTestId('add-address-anyway')).toBeOnTheScreen();
  expect(screen.getByTestId('add-address-anyway-macos')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('add-address-anyway-windows'));
  await advance();

  expect(savedServers(kv)).toMatchObject({
    entries: [
      {
        kind: 'computer',
        host: '10.30.99.9',
        port: 3389,
        hostId: null,
        os: 'windows',
        protocol: 'rdp',
      },
    ],
  });
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
  expect(navigationRef.dispatch).not.toHaveBeenCalled();
});

it('rolls back the computer saved for 输入配对码 when pairing is abandoned', async () => {
  const core = createFakeCore({ network: netCompanyVpn });
  const { kv, navigation } = await renderScreen(core);
  await findAddress('10.30.99.9');
  await emits(core, [
    probeEvent('p1', 7400, 'ok', 'newhost123'),
    probeEvent('p2', 22, 'timeout'),
  ]);
  await fireEvent.press(
    screen.getByRole('button', {
      name: [addByAddress.pair.code, addByAddress.pair.codeSubtitle].join('，'),
    }),
  );
  await advance();
  expect(savedServers(kv)?.entries).toHaveLength(1);

  // A finished pairing resets the stack; being focused again means it did not.
  const addListener = navigation.addListener as unknown as jest.Mock;
  const focus = addListener.mock.calls.filter(
    ([name]: [string]) => name === 'focus',
  );
  await act(async () => focus[focus.length - 1][1]());
  await advance();

  expect(savedServers(kv)?.entries ?? []).toEqual([]);
});
