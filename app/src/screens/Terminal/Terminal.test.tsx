import { createRef, StrictMode } from 'react';
import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { RootStackParamList } from '../../app/navigation/routes';
import HistoryRecorder from '../../features/sessions/HistoryRecorder';
import { resetSshStore, sshStore } from '../../features/sessions/sshStore';
import { MemoryKv } from '../../features/storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import * as fixtures from '../../native/rscore/fixtures';
import type { CoreEvent } from '../../native/rscore/types';
import type { TerminalPaneProps } from '../../native/terminal/TerminalPane';
import {
  resetTerminalKeyboard,
  setTerminalKeyboard,
} from '../../native/terminal/terminalKeyboard';
import { FakePlatform } from '../../native/platform/fakePlatform';
import ToastHost, { hideToast } from '../../components/ToastHost';
import { seedLocalSync } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { tokens } from '../../theme/tokens';
import TerminalScreen from './index';
import type { TerminalHandle, TerminalSlotProps } from './TerminalSlot';

const mockSent: string[] = [];
const mockSlot = jest.fn();
const mockSlotMount = jest.fn();
const mockSlotUnmount = jest.fn();
const mockSend = jest.fn();
const mockSlotPaste = jest.fn();
const mockPane = jest.fn();
const mockPaneHandle = {
  sendSequence: jest.fn(),
  copySelection: jest.fn(),
  paste: jest.fn(),
};
let mockResolveOpen: (id: string) => void;

jest.setTimeout(30000);

jest.mock('./TerminalSlot', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View } =
    jest.requireActual<typeof import('react-native')>('react-native');
  return {
    TerminalSlot: (p: TerminalSlotProps) => {
      mockSlot(p);
      React.useEffect(() => {
        mockSlotMount();
        return () => mockSlotUnmount();
      }, []);
      React.useImperativeHandle(
        p.ref,
        () => ({
          sendSequence: (value: string) => {
            mockSent.push(value);
            mockSend(p.sshId, value);
          },
          copySelection() {},
          paste: () => mockSlotPaste(p.sshId),
        }),
        [p.sshId],
      );
      return (
        <View
          accessibilityLabel={p.accessibilityLabel}
          testID={'term-' + p.sshId}
        >
          {p.accessory}
        </View>
      );
    },
  };
});

jest.mock('../../native/terminal/TerminalPane', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View } =
    jest.requireActual<typeof import('react-native')>('react-native');
  return {
    TerminalPane: (p: TerminalPaneProps) => {
      mockPane(p);
      React.useImperativeHandle(p.ref, () => mockPaneHandle, []);
      return <View testID="terminal-pane">{p.accessory}</View>;
    },
  };
});

const prod = fixtures.sshServers.find(server => server.name === 'prod-api-01')!;
const gpu = fixtures.sshServers.find(server => server.name === 'gpu-train-02')!;
const clients: QueryClient[] = [];

function fakeCore() {
  return createFakeCore();
}

/** The snippets saved on this phone. */
function snippetsKv() {
  const kv = new MemoryKv();
  seedLocalSync(kv, 'snippets', {
    version: 1,
    value: {
      v: 1,
      items: [
        { id: 'docker', label: '', cmd: 'docker ps', run: true },
        { id: 'tail', label: '', cmd: 'tail -f api.log', run: false },
      ],
    },
  });
  return kv;
}

async function renderTerminal(
  params: RootStackParamList['Terminal'] = { serverId: prod.serverId },
  opts: {
    core?: FakeCore;
    tmux?: boolean;
    strict?: boolean;
    platform?: FakePlatform;
    beforeSession?: boolean;
    waitForSnippets?: boolean;
  } = {},
) {
  const navigation = mockNavigation<'Terminal'>();
  const core = opts.core ?? fakeCore();
  const ui = (
    <>
      <HistoryRecorder />
      <TerminalScreen
        navigation={navigation}
        route={mockRoute('Terminal', params)}
      />
    </>
  );
  const rendered = await renderWithProviders(
    opts.strict ? <StrictMode>{ui}</StrictMode> : ui,
    {
      core,
      kv: snippetsKv(),
      platform: opts.platform,
      settings: {
        'terminal.tmux': opts.tmux ?? true,
        'terminal.fontSize': 13,
        'lock.beforeSession': opts.beforeSession ?? false,
      },
    },
  );
  clients.push(rendered.queryClient);
  if (opts.waitForSnippets ?? true) {
    await waitFor(() =>
      expect(screen.getByLabelText('发送命令 docker ps')).toBeOnTheScreen(),
    );
  }
  return { ...rendered, core, navigation };
}

function calls(core: FakeCore, method: string) {
  return core.calls.filter(call => call.method === method);
}

async function emit(core: FakeCore, event: CoreEvent) {
  await act(async () => core.emit(event));
}

async function connected(
  core: FakeCore,
  sshId = 'ssh1',
  serverId = prod.serverId,
) {
  await emit(core, { type: 'ssh.state', sshId, serverId, state: 'open' });
}

beforeEach(() => {
  jest.useFakeTimers();
  resetSshStore();
  resetTerminalKeyboard();
  mockSent.length = 0;
  mockSlot.mockClear();
  mockSlotMount.mockClear();
  mockSlotUnmount.mockClear();
  mockSend.mockClear();
  mockSlotPaste.mockClear();
  mockPane.mockClear();
  for (const command of Object.values(mockPaneHandle)) command.mockClear();
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

it('opens a terminal with tmux and renders its status and selected tab', async () => {
  const { core } = await renderTerminal();
  expect(calls(core, 'ssh.open')).toEqual([
    { method: 'ssh.open', args: [prod.serverId, { tmux: true }] },
  ]);
  expect(screen.getByText('正在连接…')).toBeOnTheScreen();
  await connected(core);
  expect(screen.getByText('Tailscale SSH · 免密钥')).toBeOnTheScreen();
  // T07: a first connection runs in tmux; nothing has been resumed yet.
  expect(screen.getByText('在 tmux 里运行')).toBeOnTheScreen();
  expect(screen.queryByText('已接回 tmux 会话')).toBeNull();
  expect(screen.getByLabelText('prod-api-01，已连接')).toHaveProp(
    'accessibilityState',
    { selected: true },
  );
  expect(screen.getByTestId('term-ssh1')).toBeOnTheScreen();
  expect(mockSlot).toHaveBeenLastCalledWith(
    expect.objectContaining({
      sshId: 'ssh1',
      fontSize: 13,
      theme: 'night',
      accessibilityLabel: '终端输出',
    }),
  );
});

it('honors the tmux setting', async () => {
  const { core } = await renderTerminal(undefined, { tmux: false });
  expect(calls(core, 'ssh.open')[0].args).toEqual([
    prod.serverId,
    { tmux: false },
  ]);
  await connected(core);
  expect(screen.queryByText('在 tmux 里运行')).toBeNull();
});

it('blocks snippets while connecting and sends the selected run mode once open', async () => {
  const { core } = await renderTerminal();
  await fireEvent.press(screen.getByLabelText('发送命令 docker ps'));
  expect(screen.getByText('还没连上，等连上再发')).toBeOnTheScreen();
  expect(mockSent).toEqual([]);
  await connected(core);
  await fireEvent.press(screen.getByLabelText('发送命令 docker ps'));
  expect(mockSent).toEqual(['docker ps\r']);
  await fireEvent.press(screen.getByLabelText('输入命令 tail -f api.log'));
  expect(mockSent).toEqual(['docker ps\r', 'tail -f api.log']);
  await act(async () => jest.advanceTimersByTime(3000));
  expect(screen.queryByText('还没连上，等连上再发')).toBeNull();
});

it('prefills the installation command once without executing it', async () => {
  const prefill = 'curl -fsSL https://hs.example.com/install.sh | sh';
  const { core } = await renderTerminal({ serverId: prod.serverId, prefill });
  expect(mockSent).toEqual([]);
  await connected(core);
  await connected(core);
  expect(mockSent).toEqual([prefill]);
});

it.each(['first', 'changed'] as const)(
  'routes the %s host key prompt',
  async kind => {
    const { core, navigation } = await renderTerminal();
    const event = {
      type: 'ssh.hostKeyPrompt' as const,
      sshId: 'ssh1',
      serverId: prod.serverId,
      fp: 'SHA256:x',
      keyType: 'ED25519',
      kind,
      oldFp: kind === 'changed' ? 'SHA256:old' : undefined,
    };
    await emit(core, event);
    expect(navigation.navigate).toHaveBeenCalledWith('HostKeyPrompt', {
      sshId: event.sshId,
      serverId: event.serverId,
      fp: event.fp,
      keyType: event.keyType,
      kind: event.kind,
      oldFp: event.oldFp,
    });
  },
);

it('routes Tailscale SSH confirmation and retains check status during authentication', async () => {
  const { core, navigation } = await renderTerminal();
  const params = {
    sshId: 'ssh1',
    serverId: prod.serverId,
    url: 'https://hs.example.com/a/xyz',
    user: 'root',
  };
  await emit(core, { type: 'ssh.authUrl', ...params });
  expect(navigation.navigate).toHaveBeenCalledWith('RootConfirm', params);
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh1',
    serverId: prod.serverId,
    state: 'authenticating',
  });
  expect(screen.getByText('Tailscale SSH · 等待二次确认')).toBeOnTheScreen();
  expect(screen.getByLabelText('prod-api-01，等待二次确认')).toBeOnTheScreen();
  await connected(core);
  expect(screen.getByText('Tailscale SSH · 免密钥')).toBeOnTheScreen();
});

it('shows a failed state and opens a new connection on retry', async () => {
  const { core } = await renderTerminal();
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh1',
    serverId: prod.serverId,
    state: 'failed',
    message: 'auth failed',
  });
  // T06: the core message only picks the next step; it is never shown.
  expect(
    screen.getByText('登录被拒绝，检查密钥或密码后重新连接'),
  ).toBeOnTheScreen();
  expect(screen.queryByText(/auth failed/)).toBeNull();
  await fireEvent.press(screen.getByLabelText('重新连接'));
  expect(calls(core, 'ssh.open')).toHaveLength(2);
  // An old connection's late event must not claim the newly opening tab.
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh1',
    serverId: prod.serverId,
    state: 'closed',
  });
  await connected(core, 'ssh2');
  expect(screen.getByLabelText('prod-api-01，已连接')).toBeOnTheScreen();
});

it('shows an open rejection and allows retry', async () => {
  const core = fakeCore();
  core.failNext('ssh.open', new Error('no route'));
  await renderTerminal(undefined, { core });
  expect(
    screen.getByText('连不上服务器，检查网络和地址后重新连接'),
  ).toBeOnTheScreen();
  expect(screen.queryByText(/no route/)).toBeNull();
  await fireEvent.press(screen.getByLabelText('重新连接'));
  expect(calls(core, 'ssh.open')).toHaveLength(2);
});

it('handles state events before open resolves without adding a duplicate tab', async () => {
  const core = fakeCore();
  jest.spyOn(core.ssh, 'open').mockImplementation(
    () =>
      new Promise(resolve => {
        mockResolveOpen = resolve;
      }),
  );
  await renderTerminal(undefined, { core });
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh5',
    serverId: prod.serverId,
    state: 'connecting',
  });
  expect(screen.getAllByRole('tab')).toHaveLength(1);
  expect(screen.getByText('正在连接…')).toBeOnTheScreen();
  await act(async () => mockResolveOpen('ssh5'));
  expect(screen.getAllByRole('tab')).toHaveLength(1);
  expect(screen.getByTestId('term-ssh5')).toBeOnTheScreen();
});

it('handles check events before open resolves and consecutive events before rendering', async () => {
  const core = fakeCore();
  jest.spyOn(core.ssh, 'open').mockImplementation(
    () =>
      new Promise(resolve => {
        mockResolveOpen = resolve;
      }),
  );
  const { navigation } = await renderTerminal(undefined, { core });
  await act(async () => {
    core.emit({
      type: 'ssh.authUrl',
      sshId: 'ssh5',
      serverId: prod.serverId,
      url: 'https://hs.example.com/a/xyz',
      user: 'root',
    });
    core.emit({
      type: 'ssh.state',
      sshId: 'ssh5',
      serverId: prod.serverId,
      state: 'authenticating',
    });
  });
  expect(navigation.navigate).toHaveBeenCalledWith(
    'RootConfirm',
    expect.objectContaining({ sshId: 'ssh5' }),
  );
  expect(screen.getByText('Tailscale SSH · 等待二次确认')).toBeOnTheScreen();
  await act(async () => mockResolveOpen('ssh5'));
  expect(screen.getAllByRole('tab')).toHaveLength(1);
  expect(screen.getByText('Tailscale SSH · 等待二次确认')).toBeOnTheScreen();
});

function seedGpu() {
  sshStore.apply(
    { type: 'ssh.state', sshId: 'ssh9', serverId: gpu.serverId, state: 'open' },
    fixtures.FIXTURE_NOW,
  );
}

it('attaches live sessions and selects the requested server', async () => {
  seedGpu();
  const { core } = await renderTerminal();
  expect(screen.getAllByRole('tab')).toHaveLength(2);
  expect(screen.getByLabelText('gpu-train-02，已连接')).toHaveProp(
    'accessibilityState',
    { selected: false },
  );
  expect(screen.getByLabelText('prod-api-01，正在连接')).toHaveProp(
    'accessibilityState',
    { selected: true },
  );
  await fireEvent.press(screen.getByLabelText('gpu-train-02，已连接'));
  expect(screen.getByTestId('term-ssh9')).toBeOnTheScreen();
  expect(screen.getByText('密钥登录')).toBeOnTheScreen();
  expect(calls(core, 'ssh.open')).toHaveLength(1);
});

function slotProps(sshId: string | null): TerminalSlotProps {
  return mockSlot.mock.calls
    .map(([p]) => p as TerminalSlotProps)
    .filter(p => p.sshId === sshId)
    .at(-1)!;
}

it('keeps both terminal slots mounted while transferring focus, ref and accessory', async () => {
  seedGpu();
  const { core } = await renderTerminal();
  await connected(core);
  const slots = screen.getAllByTestId(/^term-/, {
    includeHiddenElements: true,
  });
  expect(slots).toHaveLength(2);
  expect(mockSlotMount).toHaveBeenCalledTimes(2);
  expect(mockSlotUnmount).not.toHaveBeenCalled();

  const current = slotProps('ssh1');
  expect(current.active).toBe(true);
  expect(current.ref).toBeDefined();
  expect(current.accessory).toBeDefined();
  expect(slotProps('ssh9').active).toBe(false);
  expect(slotProps('ssh9').ref).toBeUndefined();
  expect(slotProps('ssh9').accessory).toBeUndefined();
  expect(screen.queryByTestId('term-ssh9')).toBeNull();
  const hidden = screen.getByTestId('term-ssh9', {
    includeHiddenElements: true,
  }).parent;
  expect(hidden).toHaveStyle({ opacity: 0, position: 'absolute' });
  expect(hidden).toHaveProp('pointerEvents', 'none');
  expect(hidden).toHaveProp('accessibilityElementsHidden', true);
  expect(hidden).toHaveProp('importantForAccessibility', 'no-hide-descendants');
  expect(
    within(screen.getByTestId('term-ssh1')).getByLabelText('命令片段'),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('发送命令 docker ps'));
  expect(mockSend).toHaveBeenLastCalledWith('ssh1', 'docker ps\r');

  await fireEvent.press(screen.getByLabelText('gpu-train-02，已连接'));
  expect(slotProps('ssh9').active).toBe(true);
  expect(slotProps('ssh9').ref).toBe(current.ref);
  expect(slotProps('ssh9').accessory).toBeDefined();
  expect(slotProps('ssh1').active).toBe(false);
  expect(slotProps('ssh1').ref).toBeUndefined();
  expect(slotProps('ssh1').accessory).toBeUndefined();
  expect(screen.queryByTestId('term-ssh1')).toBeNull();
  expect(
    within(screen.getByTestId('term-ssh9')).getByLabelText('命令片段'),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('发送命令 docker ps'));
  expect(mockSend).toHaveBeenLastCalledWith('ssh9', 'docker ps\r');

  await fireEvent.press(screen.getByLabelText('prod-api-01，已连接'));
  expect(slotProps('ssh1').active).toBe(true);
  expect(slotProps('ssh1').ref).toBe(current.ref);
  expect(slotProps('ssh9').active).toBe(false);
  expect(
    screen.getAllByTestId(/^term-/, { includeHiddenElements: true }),
  ).toEqual(slots);
  expect(mockSlotMount).toHaveBeenCalledTimes(2);
  expect(mockSlotUnmount).not.toHaveBeenCalled();
});

it('keeps an active empty terminal after the final tab closes', async () => {
  await renderTerminal();
  await fireEvent.press(screen.getByLabelText('关闭 prod-api-01 的终端'));
  expect(screen.queryAllByRole('tab')).toHaveLength(0);
  expect(screen.getAllByTestId(/^term-/)).toHaveLength(1);
  expect(screen.getByTestId('term-null')).toBeOnTheScreen();
  expect(screen.getByTestId('terminal-empty')).toBeOnTheScreen();
  expect(slotProps(null)).toMatchObject({
    active: true,
    sshId: null,
    theme: 'night',
    ref: expect.anything(),
  });
});

it('reuses a requested live sshId without opening another connection', async () => {
  seedGpu();
  const { core } = await renderTerminal({
    serverId: gpu.serverId,
    sshId: 'ssh9',
  });
  expect(calls(core, 'ssh.open')).toEqual([]);
  expect(screen.getByTestId('term-ssh9')).toBeOnTheScreen();
});

it('confirms before closing connected tabs and stays after the final tab closes', async () => {
  seedGpu();
  const { core, navigation } = await renderTerminal();
  await connected(core);
  await fireEvent.press(screen.getByLabelText('关闭 prod-api-01 的终端'));
  // T02: a connected tab asks first.
  const dialog = screen.getByTestId('terminal-close-dialog');
  expect(within(dialog).getByText('断开 prod-api-01？')).toBeOnTheScreen();
  expect(
    within(dialog).getByText(
      '会关掉这个终端的 SSH 连接，tmux 里的程序会继续运行。',
    ),
  ).toBeOnTheScreen();
  expect(calls(core, 'ssh.close')).toEqual([]);
  await fireEvent.press(screen.getByTestId('terminal-close-confirm'));
  expect(calls(core, 'ssh.close')).toEqual([
    { method: 'ssh.close', args: ['ssh1'] },
  ]);
  expect(screen.queryByTestId('terminal-close-dialog')).toBeNull();
  expect(navigation.goBack).not.toHaveBeenCalled();
  expect(screen.getAllByRole('tab')).toHaveLength(1);
  await fireEvent.press(screen.getByLabelText('关闭 gpu-train-02 的终端'));
  await fireEvent.press(screen.getByTestId('terminal-close-confirm'));
  expect(calls(core, 'ssh.close')[1].args).toEqual(['ssh9']);
  // The last tab closing keeps the page with a way to open another.
  expect(navigation.goBack).not.toHaveBeenCalled();
  expect(screen.getByTestId('terminal-empty')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('terminal-empty-new'));
  expect(screen.getByTestId('new-terminal-sheet')).toBeOnTheScreen();
});

it('keeps a connected tab when its close is cancelled', async () => {
  const { core } = await renderTerminal(undefined, { tmux: false });
  await connected(core);
  await fireEvent.press(screen.getByLabelText('关闭 prod-api-01 的终端'));
  expect(screen.getByText('会关掉这个终端的 SSH 连接。')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText('取消'));
  expect(screen.queryByTestId('terminal-close-dialog')).toBeNull();
  expect(calls(core, 'ssh.close')).toEqual([]);
  expect(screen.getAllByRole('tab')).toHaveLength(1);
});

it('lists the four servers and opens the selected one in a new tab', async () => {
  const { core } = await renderTerminal();
  await fireEvent.press(screen.getByLabelText('新建终端'));
  const sheet = within(screen.getByTestId('new-terminal-sheet'));
  expect(sheet.getByRole('header')).toHaveTextContent('新建终端');
  for (const server of fixtures.sshServers) {
    expect(sheet.getByLabelText(server.name)).toBeOnTheScreen();
  }
  expect(sheet.getByText('Ubuntu 24.04 · Tailscale SSH')).toBeOnTheScreen();
  expect(
    sheet.getByText('Ubuntu 24.04 · 密码登录 · 经 jump-01'),
  ).toBeOnTheScreen();
  await fireEvent.press(sheet.getByLabelText(gpu.name));
  expect(screen.queryByTestId('new-terminal-sheet')).toBeNull();
  expect(screen.getAllByRole('tab')).toHaveLength(2);
  expect(calls(core, 'ssh.open')[1].args).toEqual([
    gpu.serverId,
    { tmux: true },
  ]);
});

it('navigates to AddServer from the new terminal sheet', async () => {
  const { navigation } = await renderTerminal();
  await fireEvent.press(screen.getByLabelText('新建终端'));
  await fireEvent.press(screen.getByLabelText('添加服务器'));
  expect(navigation.navigate).toHaveBeenCalledWith('AddServer');
  expect(screen.queryByTestId('new-terminal-sheet')).toBeNull();
});

it('opens the existing snippets editor', async () => {
  await renderTerminal();
  await fireEvent.press(screen.getByLabelText('编辑片段'));
  expect(screen.getByRole('header', { name: '命令片段' })).toBeOnTheScreen();
});

it('hides snippet editing and keeps the toast under the header while the keyboard is visible', async () => {
  await renderTerminal();
  expect(screen.getByLabelText('编辑片段')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('发送命令 docker ps'));
  // T25: the toast sits under the header, clear of the snippet bar:
  // the mocked top inset 47 + header 52 + a 4pt gap.
  expect(screen.getByTestId('toast').parent).toHaveStyle({ top: 103 });

  await act(async () => setTerminalKeyboard(true, 336));
  expect(screen.queryByLabelText('编辑片段')).toBeNull();
  expect(
    within(screen.getByTestId('term-ssh1')).getByLabelText(
      '发送命令 docker ps',
    ),
  ).toBeOnTheScreen();
  expect(screen.getByTestId('toast').parent).toHaveStyle({ top: 103 });

  await act(async () => resetTerminalKeyboard());
  expect(screen.getByLabelText('编辑片段')).toBeOnTheScreen();
  expect(screen.getByTestId('toast').parent).toHaveStyle({ top: 103 });
});

it.each([false, true])(
  'offers an add snippet pill with an empty list and keyboard visible=%s',
  async visible => {
    setTerminalKeyboard(visible, visible ? 336 : 0);
    const core = createFakeCore();
    const navigation = mockNavigation<'Terminal'>();
    const result = await renderWithProviders(
      <>
        <HistoryRecorder />
        <TerminalScreen
          navigation={navigation}
          route={mockRoute('Terminal', { serverId: prod.serverId })}
        />
      </>,
      { core },
    );
    clients.push(result.queryClient);
    expect(screen.getByLabelText('添加命令片段')).toBeOnTheScreen();
    expect(screen.queryByLabelText('编辑片段')).toBeNull();
    await fireEvent.press(screen.getByLabelText('添加命令片段'));
    expect(screen.getByRole('header', { name: '命令片段' })).toBeOnTheScreen();
  },
);

it('ignores state and confirmation events belonging to another server', async () => {
  const { core, navigation } = await renderTerminal();
  await emit(core, {
    type: 'ssh.state',
    sshId: 'other',
    serverId: gpu.serverId,
    state: 'open',
  });
  await emit(core, {
    type: 'ssh.authUrl',
    sshId: 'other',
    serverId: gpu.serverId,
    url: 'https://hs.example.com/a/xyz',
    user: 'root',
  });
  await emit(core, {
    type: 'ssh.hostKeyPrompt',
    sshId: 'other',
    serverId: gpu.serverId,
    fp: 'SHA256:x',
    keyType: 'ED25519',
    kind: 'first',
  });
  expect(screen.getByText('正在连接…')).toBeOnTheScreen();
  expect(screen.getAllByRole('tab')).toHaveLength(1);
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('leaves SSH running when navigating back or unmounting', async () => {
  const { core, navigation, unmount } = await renderTerminal();
  await connected(core);
  await fireEvent.press(screen.getByLabelText('返回'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
  await unmount();
  expect(calls(core, 'ssh.close')).toEqual([]);
});

it('does not close a pending open when the page unmounts', async () => {
  const core = fakeCore();
  jest.spyOn(core.ssh, 'open').mockImplementation(
    () =>
      new Promise(resolve => {
        mockResolveOpen = resolve;
      }),
  );
  const { unmount } = await renderTerminal(undefined, { core });
  await unmount();
  await act(async () => mockResolveOpen('ssh5'));
  expect(calls(core, 'ssh.close')).toEqual([]);
});

it('closes a pending connection once it returns after an explicit tab close', async () => {
  const core = fakeCore();
  jest.spyOn(core.ssh, 'open').mockImplementation(
    () =>
      new Promise(resolve => {
        mockResolveOpen = resolve;
      }),
  );
  const { navigation } = await renderTerminal(undefined, { core });
  // A tab still connecting closes without asking.
  await fireEvent.press(screen.getByLabelText('关闭 prod-api-01 的终端'));
  expect(screen.queryByTestId('terminal-close-dialog')).toBeNull();
  expect(navigation.goBack).not.toHaveBeenCalled();
  await act(async () => mockResolveOpen('ssh5'));
  expect(calls(core, 'ssh.close')).toEqual([
    { method: 'ssh.close', args: ['ssh5'] },
  ]);
});

it('does not duplicate an open request or prefill during StrictMode effects', async () => {
  const { core } = await renderTerminal(
    { serverId: prod.serverId, prefill: 'htop' },
    { strict: true },
  );
  expect(calls(core, 'ssh.open')).toHaveLength(1);
  await connected(core);
  expect(mockSent).toEqual(['htop']);
});

it.each([
  ['authenticating', '正在登录…', '正在登录'],
  ['closed', '已断开', '已断开'],
  ['failed', '连接失败', '连接失败'],
] as const)(
  'renders the %s state without a message',
  async (state, label, phase) => {
    const { core } = await renderTerminal();
    await emit(core, {
      type: 'ssh.state',
      sshId: 'ssh1',
      serverId: prod.serverId,
      state,
    });
    expect(screen.getByText(label)).toBeOnTheScreen();
    expect(screen.getByLabelText(`prod-api-01，${phase}`)).toBeOnTheScreen();
    expect(screen.queryByLabelText('重新连接') !== null).toBe(
      state !== 'authenticating',
    );
  },
);

it('shows the password login and jump host for a manual server', async () => {
  const serverId = 'manual-build-srv';
  const { core } = await renderTerminal({ serverId });
  await connected(core, 'ssh1', serverId);
  expect(screen.getByText('密码登录 · 经 jump-01')).toBeOnTheScreen();
});

it('falls back to SSH when server metadata is unavailable', async () => {
  const serverId = 'removed-server';
  const { core } = await renderTerminal({ serverId });
  await connected(core, 'ssh1', serverId);
  expect(screen.getByText('SSH')).toBeOnTheScreen();
  expect(screen.getByLabelText('removed-server，已连接')).toBeOnTheScreen();
});

it('waits for a prefilled tab to be selected and never resends after reconnect', async () => {
  seedGpu();
  const { core } = await renderTerminal({
    serverId: prod.serverId,
    prefill: 'htop',
  });
  await fireEvent.press(screen.getByLabelText('gpu-train-02，已连接'));
  await connected(core);
  expect(mockSent).toEqual([]);
  await fireEvent.press(screen.getByLabelText('prod-api-01，已连接'));
  expect(mockSent).toEqual(['htop']);
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh1',
    serverId: prod.serverId,
    state: 'closed',
  });
  await fireEvent.press(screen.getByLabelText('重新连接'));
  await connected(core, 'ssh2');
  expect(mockSent).toEqual(['htop']);
});

it('closes only once if an early event identifies a pending connection', async () => {
  const core = fakeCore();
  jest.spyOn(core.ssh, 'open').mockImplementation(
    () =>
      new Promise(resolve => {
        mockResolveOpen = resolve;
      }),
  );
  await renderTerminal(undefined, { core });
  await connected(core, 'ssh5');
  await fireEvent.press(screen.getByLabelText('关闭 prod-api-01 的终端'));
  await fireEvent.press(screen.getByTestId('terminal-close-confirm'));
  await act(async () => mockResolveOpen('ssh5'));
  expect(calls(core, 'ssh.close')).toEqual([
    { method: 'ssh.close', args: ['ssh5'] },
  ]);
});

it('forwards slot props, accessory and the ref interface to TerminalPane', async () => {
  const { TerminalSlot } =
    jest.requireActual<typeof import('./TerminalSlot')>('./TerminalSlot');
  const ref = createRef<TerminalHandle>();
  const accessory = <></>;
  const result = await renderWithProviders(
    <TerminalSlot
      active
      accessory={accessory}
      sshId={null}
      fontSize={13}
      theme="night"
      accessibilityLabel="终端输出"
      ref={ref}
    />,
  );
  clients.push(result.queryClient);
  expect(screen.getByTestId('terminal-pane')).toBeOnTheScreen();
  expect(mockPane).toHaveBeenLastCalledWith({
    active: true,
    accessory,
    sshId: null,
    fontSize: 13,
    theme: 'night',
    accessibilityLabel: '终端输出',
    ref,
  });
  expect(ref.current).toBe(mockPaneHandle);
  ref.current?.sendSequence('htop');
  ref.current?.copySelection();
  ref.current?.paste();
  expect(mockPaneHandle.sendSequence).toHaveBeenCalledWith('htop');
  expect(mockPaneHandle.copySelection).toHaveBeenCalledTimes(1);
  expect(mockPaneHandle.paste).toHaveBeenCalledTimes(1);
});

it('asks for the App lock before opening an SSH connection', async () => {
  const platform = new FakePlatform();
  let resolveAuth: (result: 'ok' | 'cancelled') => void = () => {};
  jest.spyOn(platform, 'authenticate').mockImplementation(
    () =>
      new Promise(resolve => {
        resolveAuth = resolve;
      }),
  );
  const { core } = await renderTerminal(undefined, {
    platform,
    beforeSession: true,
    waitForSnippets: false,
  });
  // S03: nothing connects and no output shows until the lock is passed.
  await waitFor(() =>
    expect(platform.authenticate).toHaveBeenCalledWith(
      '打开 prod-api-01 的终端',
    ),
  );
  expect(screen.getByTestId('terminal-locked')).toBeOnTheScreen();
  expect(screen.queryByTestId('term-ssh1')).toBeNull();
  expect(calls(core, 'ssh.open')).toEqual([]);
  await act(async () => resolveAuth('ok'));
  expect(calls(core, 'ssh.open')).toHaveLength(1);
  expect(screen.queryByTestId('terminal-locked')).toBeNull();
  expect(platform.authenticate).toHaveBeenCalledTimes(1);
});

it.each(['cancelled', 'failed'] as const)(
  'leaves the terminal without connecting when the App lock is %s',
  async result => {
    const platform = new FakePlatform();
    platform.authResult = result;
    const { core, navigation } = await renderTerminal(undefined, {
      platform,
      beforeSession: true,
      waitForSnippets: false,
    });
    await waitFor(() => expect(navigation.goBack).toHaveBeenCalledTimes(1));
    expect(calls(core, 'ssh.open')).toEqual([]);
  },
);

it('does not ask for the App lock when it is off', async () => {
  const platform = new FakePlatform();
  const { core } = await renderTerminal(undefined, { platform });
  expect(platform.calls.filter(call => call.method === 'authenticate')).toEqual(
    [],
  );
  expect(calls(core, 'ssh.open')).toHaveLength(1);
});

it('shows copied feedback and pastes into a connected terminal only', async () => {
  const { core } = await renderTerminal();
  // T05: paste waits for a connection.
  expect(screen.getByTestId('terminal-paste')).toBeDisabled();
  await connected(core);
  await fireEvent.press(screen.getByLabelText('粘贴'));
  expect(mockSlotPaste).toHaveBeenCalledWith('ssh1');
  await act(async () => slotProps('ssh1').onCopied?.());
  expect(screen.getByTestId('toast')).toHaveTextContent('已复制');
});

it('numbers tabs that share a server name', async () => {
  const { core } = await renderTerminal();
  await connected(core);
  await fireEvent.press(screen.getByLabelText('新建终端'));
  await fireEvent.press(
    within(screen.getByTestId('new-terminal-sheet')).getByLabelText(prod.name),
  );
  // T16: two terminals on one server are told apart.
  expect(screen.getByLabelText('prod-api-01，已连接')).toBeOnTheScreen();
  expect(screen.getByLabelText('prod-api-01 2，正在连接')).toBeOnTheScreen();
});

it('marks servers that already have a terminal in the new terminal sheet', async () => {
  await renderTerminal();
  await fireEvent.press(screen.getByLabelText('新建终端'));
  const sheet = within(screen.getByTestId('new-terminal-sheet'));
  // T29: the open server says so; its label stays the server name.
  expect(sheet.getByLabelText(prod.name)).toHaveTextContent(/已打开/);
  expect(sheet.getByLabelText(gpu.name)).not.toHaveTextContent(/已打开/);
});

it('queues a second host key prompt until the first is answered', async () => {
  seedGpu();
  const core = fakeCore();
  const { navigation } = await renderTerminal(undefined, { core });
  await fireEvent.press(screen.getByLabelText('新建终端'));
  await fireEvent.press(
    within(screen.getByTestId('new-terminal-sheet')).getByLabelText(gpu.name),
  );
  const prompt = (sshId: string, serverId: string) => ({
    type: 'ssh.hostKeyPrompt' as const,
    sshId,
    serverId,
    fp: 'SHA256:' + sshId,
    keyType: 'ED25519',
    kind: 'first' as const,
  });
  await emit(core, prompt('ssh1', prod.serverId));
  await emit(core, prompt('ssh2', gpu.serverId));
  // T20: the second prompt waits instead of replacing the first.
  expect(navigation.navigate).toHaveBeenCalledTimes(1);
  expect(navigation.navigate).toHaveBeenLastCalledWith(
    'HostKeyPrompt',
    expect.objectContaining({ sshId: 'ssh1' }),
  );
  const focus = listener(navigation, 'focus');
  await act(async () => focus());
  expect(navigation.navigate).toHaveBeenCalledTimes(2);
  expect(navigation.navigate).toHaveBeenLastCalledWith(
    'HostKeyPrompt',
    expect.objectContaining({ sshId: 'ssh2' }),
  );
  await act(async () => focus());
  expect(navigation.navigate).toHaveBeenCalledTimes(2);
});

it('says the terminal stays connected when leaving with an open tab', async () => {
  const { core, navigation } = await renderTerminal();
  await renderToastHost();
  const leave = listener(navigation, 'beforeRemove');
  // T15: nothing to say while nothing is connected.
  await act(async () => leave());
  expect(screen.queryByTestId('global-toast')).toBeNull();
  await connected(core);
  await act(async () => leave());
  expect(screen.getByTestId('global-toast')).toHaveTextContent(
    '终端在后台保持连接，回来就能接着用',
  );
});

function listener(
  navigation: ReturnType<typeof mockNavigation<'Terminal'>>,
  name: string,
): () => void {
  const added = (navigation.addListener as unknown as jest.Mock).mock.calls as [
    string,
    () => void,
  ][];
  const found = added.filter(([event]) => event === name).at(-1);
  if (found === undefined) {
    throw new Error(`no ${name} listener`);
  }
  return found[1];
}

async function renderToastHost() {
  const result = await renderWithProviders(<ToastHost />);
  clients.push(result.queryClient);
}

const touch = { persist: () => {}, nativeEvent: {}, currentTarget: 1 };

it('dims snippet pills until connected and shows pressed states', async () => {
  const { core } = await renderTerminal();
  const pill = screen.getByTestId('snippet-pill-docker');
  // T22: not connected yet, so the pills read as unavailable.
  expect(within(pill).getByText('docker ps')).toHaveStyle({
    color: tokens.text3,
  });
  // T21: a long command stays a short pill.
  expect(pill).toHaveStyle({ maxWidth: 200 });
  await connected(core);
  expect(
    within(screen.getByTestId('snippet-pill-docker')).getByText('docker ps'),
  ).toHaveStyle({ color: tokens.text });
  await fireEvent(
    screen.getByTestId('snippet-pill-docker'),
    'responderGrant',
    touch,
  );
  expect(screen.getByTestId('snippet-pill-docker')).toHaveStyle({
    opacity: 0.6,
  });
});

it('offers a retry when the server list fails in the new terminal sheet', async () => {
  const core = fakeCore();
  // Fails on every read until the retry, including the sheet's own refetch.
  const servers = jest
    .spyOn(core.ssh, 'servers')
    .mockRejectedValue(new Error('offline'));
  await renderTerminal(undefined, { core });
  await fireEvent.press(screen.getByLabelText('新建终端'));
  const sheet = within(screen.getByTestId('new-terminal-sheet'));
  // T29: a failed list is not an empty one.
  await waitFor(() =>
    expect(sheet.getByTestId('new-terminal-failed')).toBeOnTheScreen(),
  );
  expect(sheet.getByText('服务器列表没加载出来')).toBeOnTheScreen();
  servers.mockRestore();
  await fireEvent.press(sheet.getByTestId('new-terminal-retry'));
  await waitFor(() => expect(sheet.getByLabelText(gpu.name)).toBeOnTheScreen());
  expect(sheet.queryByTestId('new-terminal-failed')).toBeNull();
});
