import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import type { ReactElement } from 'react';
import type { QueryClient } from '@tanstack/react-query';
import { resetDenied } from '../../../features/errors/deniedQuota';
import { resetTailnetSwitchStore } from '../../../features/tailnet/switchStore';
import { createFakeCore, type FakeCore } from '../../../native/rscore/fakeCore';
import { computers, sshServers } from '../../../native/rscore/fixtures';
import type { Computer } from '../../../native/rscore/types';
import { renderWithProviders } from '../../../test/render';
import { hideToast, ToastHost } from '../../../components/ToastHost';
import type { BoardProps, EndActions } from '../boardTypes';
import DeniedBoard from './DeniedBoard';
import PermissionBoard from './PermissionBoard';
import PolicyBoard from './PolicyBoard';
import PortBoard from './PortBoard';
import VpnBoard from './VpnBoard';

const STUDIO = 'h7v2m9q3k8x4tb1a';
const OFFICE = 'w3k8p2x7m4q9tb6c';
const hidden = { includeHiddenElements: true } as const;

const queryClients: QueryClient[] = [];

function actions(): EndActions & Record<keyof EndActions, jest.Mock> {
  return {
    retry: jest.fn(),
    openSession: jest.fn(),
    back: jest.fn(),
    openTerminal: jest.fn(),
    openScreenSharing: jest.fn(async () => undefined),
    copyText: jest.fn(),
    copyCode: jest.fn(),
    pair: jest.fn(),
    openDiagnostics: jest.fn(),
    toast: jest.fn(),
  } as never;
}

function illustration() {
  return screen.getByTestId('session-ended-illustration', hidden);
}

function hostFor(hostId: string): Computer {
  const host = computers.find(item => item.hostId === hostId);
  if (host === undefined) {
    throw new Error(`Unknown host ${hostId}`);
  }
  return host;
}

async function renderBoard(
  Board: (props: BoardProps) => ReactElement,
  opts: {
    host?: Computer;
    code: number;
    params?: BoardProps['params'];
    core?: FakeCore;
    settings?: {
      'hosts.prefs'?: Record<string, { routePolicy?: 'auto' | 'tailnet' }>;
      'tailnet.controlUrl'?: string | null;
      'tailnet.enabled'?: boolean;
    };
  },
) {
  const boardActions = actions();
  const host = opts.host ?? hostFor(STUDIO);
  const params = opts.params ?? { hostId: host.hostId };
  const view = await renderWithProviders(
    <Board
      actions={boardActions}
      code={opts.code}
      host={host}
      hostName={host.name}
      os={host.os}
      params={params}
    />,
    // The boards' Tailscale actions need 外网访问 on.
    { settings: { 'tailnet.enabled': true, ...opts.settings }, core: opts.core },
  );
  queryClients.push(view.queryClient);
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, actions: boardActions };
}

afterEach(async () => {
  await act(async () => hideToast());
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
  jest.useRealTimers();
});

beforeEach(() => {
  resetDenied();
  jest.useFakeTimers();
  jest.setSystemTime(new Date('2026-10-21T10:23:00Z'));
});

describe('PermissionBoard (101)', () => {
  it('shows the screen-recording path and wires every action', async () => {
    const core = createFakeCore({
      sshServers: [
        ...sshServers,
        { ...sshServers[0]!, serverId: 'studio-ssh', host: 'studio-mac' },
      ],
    });
    const { actions: boardActions } = await renderBoard(PermissionBoard, {
      code: 101,
      core,
    });
    expect(
      screen.getByText('Studio Mac mini 还没有授予屏幕录制权限'),
    ).toBeOnTheScreen();
    expect(screen.getByLabelText('设置路径')).toHaveTextContent(
      /系统设置\s*›\s*隐私与安全性\s*›\s*屏幕与系统录音/,
    );

    await fireEvent.press(screen.getByText('已打开，重试'));
    expect(boardActions.retry).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByText('改用屏幕共享'));
    expect(boardActions.openScreenSharing).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/也可以先用终端连上它/)).toBeOnTheScreen();
    await fireEvent.press(screen.getByText('改用终端'));
    expect(boardActions.openTerminal).toHaveBeenCalledWith('studio-ssh');
    await fireEvent.press(screen.getByLabelText('复制错误码 RS-E101'));
    expect(boardActions.copyCode).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByText('返回'));
    expect(boardActions.back).toHaveBeenCalledTimes(1);
  });

  it('switches to the accessibility wording', async () => {
    await renderBoard(PermissionBoard, {
      code: 101,
      params: { hostId: STUDIO, detail: { perm: 'accessibility' } },
    });
    expect(
      screen.getByText('Studio Mac mini 还没有授予辅助功能权限'),
    ).toBeOnTheScreen();
    expect(screen.getByLabelText('设置路径')).toHaveTextContent(/辅助功能$/);
  });

  it('offers no terminal without an SSH server for the computer (V58)', async () => {
    await renderBoard(PermissionBoard, { code: 101 });
    expect(screen.queryByText('改用终端')).toBeNull();
    expect(screen.queryByText(/终端/)).toBeNull();
    expect(screen.getByText('改用屏幕共享')).toBeOnTheScreen();
  });

  it('names both tools when the system is unknown (V60)', async () => {
    const boardActions = actions();
    const view = await renderWithProviders(
      <PermissionBoard
        actions={boardActions}
        code={101}
        host={undefined}
        hostName="这台电脑"
        os={null}
        params={{ hostId: 'unknown-host' }}
      />,
    );
    queryClients.push(view.queryClient);
    expect(screen.getByText('改用屏幕共享或远程桌面')).toBeOnTheScreen();
    expect(
      screen.getByText(/打开开关后，系统会要求重启 Host/, hidden),
    ).toBeTruthy();
    expect(screen.queryByText(/Mac/, hidden)).toBeNull();
  });

  it('uses remote desktop for a Windows host', async () => {
    await renderBoard(PermissionBoard, { code: 101, host: hostFor(OFFICE) });
    expect(screen.getByText('改用远程桌面')).toBeOnTheScreen();
  });

  it('shows that macOS opened the settings pane', async () => {
    await renderBoard(PermissionBoard, {
      code: 101,
      params: { hostId: STUDIO, detail: { opened: 'true' } },
    });
    expect(screen.getByText('已在 Mac 上弹出这个设置页')).toBeOnTheScreen();
  });
});

describe('PolicyBoard (130)', () => {
  it('shows the generated ACL and confirms copying', async () => {
    const view = await renderBoard(PolicyBoard, { code: 130 });
    expect(
      within(illustration()).getByText(/"tag:rs-host:7400"/, hidden),
    ).toBeOnTheScreen();
    expect(
      within(illustration()).getByText(/"<你的账号>"\]/, hidden),
    ).toBeOnTheScreen();
    expect(screen.getByText(/换上你的账号/)).toBeOnTheScreen();
    expect(screen.queryByText(/alice/, hidden)).toBeNull();

    await fireEvent.press(screen.getByText('复制策略片段'));
    expect(view.actions.copyText).toHaveBeenCalledWith(
      expect.stringContaining('tag:rs-host:7400'),
      '已复制策略片段',
    );
    expect(screen.getByText('已复制')).toBeOnTheScreen();
    await act(async () => {
      await jest.advanceTimersByTimeAsync(2000);
    });
    expect(screen.getByText('复制策略片段')).toBeOnTheScreen();
  });

  it('switches to allowed_users guidance', async () => {
    await renderBoard(PolicyBoard, {
      code: 130,
      params: { hostId: STUDIO, detail: { reason: 'allowed_users' } },
    });
    await waitFor(() => {
      expect(
        screen.getByText('Studio Mac mini 没有允许你的账号连接'),
      ).toBeOnTheScreen();
    });
    expect(screen.queryByText('复制策略片段')).toBeNull();
    expect(
      within(illustration()).getByText('Host 设置 › 访问 › 允许的用户', hidden),
    ).toBeOnTheScreen();
  });
});

describe('DeniedBoard (140)', () => {
  it('shows the rejection and retries as viewer', async () => {
    const view = await renderBoard(DeniedBoard, { code: 140 });
    expect(
      screen.getByText('对方在 Studio Mac mini 上拒绝了这次连接'),
    ).toBeOnTheScreen();
    expect(screen.getByText('10 分钟内还能再请求 2 次')).toBeOnTheScreen();
    expect(
      within(illustration()).getByText('我的 iPhone 想控制这台 Mac', hidden),
    ).toBeOnTheScreen();
    expect(screen.queryByText(/alice|小明的 iPad/, hidden)).toBeNull();

    await fireEvent.press(screen.getByText('只请求观看'));
    expect(view.actions.retry).toHaveBeenCalledWith({ viewOnly: true });
  });

  it('disables both requests after three rejections', async () => {
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const view = await renderBoard(DeniedBoard, { code: 140 });
      if (attempt < 2) {
        await view.unmount();
      }
    }
    expect(screen.getByText('请求太频繁，10 分钟后再试')).toBeOnTheScreen();
    expect(screen.getByText('再次请求')).toBeDisabled();
    expect(screen.getByText('只请求观看')).toBeDisabled();
  });

  it('shows the timeout copy without the rejection marker', async () => {
    await renderBoard(DeniedBoard, {
      code: 140,
      params: { hostId: STUDIO, detail: { reason: 'timeout' } },
    });
    expect(
      screen.getByText('Studio Mac mini 上 30 秒没人回应'),
    ).toBeOnTheScreen();
    expect(screen.queryByText('对方点了这里')).toBeNull();
    expect(
      within(illustration()).getByText('10:23 · 30 秒没人点', hidden),
    ).toBeOnTheScreen();
  });

  it('honours retries_left from the host', async () => {
    await renderBoard(DeniedBoard, {
      code: 140,
      params: { hostId: STUDIO, detail: { retries_left: '1' } },
    });
    expect(screen.getByText('10 分钟内还能再请求 1 次')).toBeOnTheScreen();
  });
});

describe('VpnBoard (160)', () => {
  it('enables the embedded connection and reconnects after switching (V54)', async () => {
    resetTailnetSwitchStore();
    const view = await renderBoard(VpnBoard, {
      code: 160,
      core: createFakeCore({ tailnetAvailable: true }),
      settings: { 'tailnet.controlUrl': 'https://hs.example.com' },
    });

    expect(screen.getByLabelText('改用 App 内连接')).not.toBeDisabled();
    expect(
      screen.getByLabelText('改用 App 内连接').props.accessibilityHint,
    ).toBeUndefined();
    expect(
      within(illustration()).getByText(
        'App 内连接不占系统 VPN，可以和公司 VPN 同时开',
        hidden,
      ),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByLabelText('改用 App 内连接'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(view.settingsStore.get('tailnet.mode')).toBe('embedded');
    expect(view.actions.retry).toHaveBeenCalledTimes(1);
    expect(view.actions.back).not.toHaveBeenCalled();
    expect(screen.queryByTestId('tailnet-control-url-sheet')).toBeNull();
  });

  it('asks for an address and reconnects only after the sheet switches', async () => {
    resetTailnetSwitchStore();
    const view = await renderBoard(VpnBoard, {
      code: 160,
      core: createFakeCore({ tailnetAvailable: true }),
    });

    await fireEvent.press(screen.getByLabelText('改用 App 内连接'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByTestId('tailnet-control-url-sheet')).toBeOnTheScreen();
    expect(view.actions.retry).not.toHaveBeenCalled();
    await fireEvent.press(
      screen.getByTestId('tailnet-control-url-sheet-close'),
    );
    expect(screen.queryByTestId('tailnet-control-url-sheet')).toBeNull();
    expect(view.settingsStore.get('tailnet.mode')).toBe('system');
    expect(view.actions.retry).not.toHaveBeenCalled();

    await fireEvent.press(screen.getByLabelText('改用 App 内连接'));
    await act(async () => {
      await Promise.resolve();
    });
    await fireEvent.changeText(
      screen.getByTestId('tailnet-control-url-input'),
      'hs.example.com',
    );
    await fireEvent.press(screen.getByText('继续'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.queryByTestId('tailnet-control-url-sheet')).toBeNull();
    expect(view.settingsStore.get('tailnet.mode')).toBe('embedded');
    expect(view.actions.retry).toHaveBeenCalledTimes(1);
    expect(view.actions.back).not.toHaveBeenCalled();
  });

  it('shows the intranet address and persists the tailnet preference', async () => {
    const view = await renderBoard(VpnBoard, {
      code: 160,
      host: hostFor(OFFICE),
    });
    expect(
      screen.getByText('10.30.12.48 只能在公司网络里访问'),
    ).toBeOnTheScreen();
    expect(
      within(illustration()).getByText(
        '内网 10.30.12.48 · 需要公司 VPN',
        hidden,
      ),
    ).toBeOnTheScreen();
    // V54: no greyed-out button that can never work.
    expect(screen.queryByLabelText('改用 App 内连接')).toBeNull();
    // Nor a note about the in-app connection this build does not have.
    expect(screen.queryByText(/App 内连接/, hidden)).toBeNull();
    expect(screen.queryByText(/tailnet|上海/, hidden)).toBeNull();

    await fireEvent.press(screen.getByText('改走 Tailscale'));
    expect(view.settingsStore.get('hosts.prefs')).toMatchObject({
      [OFFICE]: { routePolicy: 'tailnet' },
    });
    expect(view.actions.retry).toHaveBeenCalledTimes(1);
  });

  it('says the Tailscale route sticks and can undo it (V53)', async () => {
    const boardActions = actions();
    const host = hostFor(OFFICE);
    const view = await renderWithProviders(
      <>
        <VpnBoard
          actions={boardActions}
          code={160}
          host={host}
          hostName={host.name}
          os={host.os}
          params={{ hostId: OFFICE }}
        />
        <ToastHost />
      </>,
      { settings: { 'tailnet.enabled': true } },
    );
    queryClients.push(view.queryClient);

    await fireEvent.press(screen.getByText('改走 Tailscale'));
    expect(screen.getByTestId('global-toast')).toHaveTextContent(
      /以后连这台电脑都走 Tailscale/,
    );
    expect(view.settingsStore.get('hosts.prefs')[OFFICE]?.routePolicy).toBe(
      'tailnet',
    );
    await fireEvent.press(screen.getByTestId('global-toast-action'));
    expect(view.settingsStore.get('hosts.prefs')[OFFICE]?.routePolicy).toBe(
      'auto',
    );
  });

  it('only asks for the company VPN while Tailscale is switched off', async () => {
    resetTailnetSwitchStore();
    const view = await renderBoard(VpnBoard, {
      code: 160,
      host: hostFor(OFFICE),
      core: createFakeCore({ tailnetAvailable: true }),
      settings: {
        'tailnet.enabled': false,
        'tailnet.controlUrl': 'https://hs.example.com',
      },
    });
    expect(
      screen.getByText('10.30.12.48 只能在公司网络里访问'),
    ).toBeOnTheScreen();
    expect(screen.getByText('我已连接，重试')).toBeOnTheScreen();
    expect(screen.queryByText('改走 Tailscale')).toBeNull();
    expect(screen.queryByLabelText('改用 App 内连接')).toBeNull();
    expect(
      screen.queryAllByText(/Tailscale|tailnet|App 内连接/, hidden),
    ).toHaveLength(0);
    expect(view.settingsStore.get('hosts.prefs')[OFFICE]).toBeUndefined();
  });

  it('reports tailnet status from the network judge', async () => {
    await renderBoard(VpnBoard, { code: 160, host: hostFor(OFFICE) });
    await waitFor(() => {
      expect(
        within(illustration()).getByText('可以走', hidden),
      ).toBeOnTheScreen();
    });
  });
});

describe('PortBoard (161)', () => {
  it('renders diagnostics and copies them as labelled lines', async () => {
    const view = await renderBoard(PortBoard, {
      code: 161,
      host: hostFor(OFFICE),
      params: {
        hostId: OFFICE,
        detail: {
          udp: 'timeout',
          tcp: 'refused',
          host_intranet: 'off',
          ping_ms: '21',
        },
      },
    });
    expect(within(illustration()).getByText('超时', hidden)).toBeOnTheScreen();
    expect(
      within(illustration()).getByText('被拒绝', hidden),
    ).toBeOnTheScreen();
    expect(
      within(illustration()).getByText('已关闭', hidden),
    ).toBeOnTheScreen();
    expect(within(illustration()).getByText('21 ms', hidden)).toBeOnTheScreen();
    expect(
      screen.getByText(
        '公司 VPN 已连上，网络也通，但 Host 的端口连不上。TCP 被直接拒绝，多半是那台电脑没打开「允许通过内网 IP 连接」。',
      ),
    ).toBeOnTheScreen();
    expect(screen.getByText('连不上 工位 Windows 上的 Host')).toBeOnTheScreen();

    await fireEvent.press(screen.getByText('复制诊断信息'));
    expect(view.actions.copyText).toHaveBeenCalledWith(
      expect.stringContaining('UDP 7400：超时'),
      '已复制诊断信息',
    );

    await fireEvent.press(screen.getByText('在电脑上怎么打开'));
    expect(screen.getByText('在那台电脑上打开内网连接')).toBeOnTheScreen();
  });

  it('retries from the port board and offers remote desktop (V30)', async () => {
    const view = await renderBoard(PortBoard, {
      code: 161,
      host: hostFor(OFFICE),
      params: { hostId: OFFICE, detail: {} },
    });
    await fireEvent.press(screen.getByText('重试'));
    expect(view.actions.retry).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByText('改用远程桌面'));
    expect(view.actions.openScreenSharing).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByText('返回'));
    expect(view.actions.back).toHaveBeenCalledTimes(1);
  });

  it('falls back to two timeout rows for an empty detail', async () => {
    await renderBoard(PortBoard, {
      code: 161,
      host: hostFor(OFFICE),
      params: { hostId: OFFICE, detail: {} },
    });
    expect(within(illustration()).getAllByText('超时', hidden)).toHaveLength(2);
    expect(within(illustration()).queryByText('公司 VPN', hidden)).toBeNull();
  });
});
