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
import { computers, FIXTURE_NOW } from '../../../native/rscore/fixtures';
import { createFakeCore } from '../../../native/rscore/fakeCore';
import type { Computer } from '../../../native/rscore/types';
import type { LocalDirectorySeed } from '../../../test/localDirectory';
import { renderWithProviders } from '../../../test/render';
import type { BoardProps, EndActions } from '../boardTypes';
import FingerprintBoard from './FingerprintBoard';
import NoHostBoard from './NoHostBoard';
import OfflineBoard from './OfflineBoard';

declare const Buffer: {
  from(input: number[] | Uint8Array): {
    toString(encoding: 'base64url'): string;
  };
};

const STUDIO = 'h7v2m9q3k8x4tb1a';
const HOME_PC = 'd2n6c4v8b1x5za7e';
const MACBOOK = 'm5q1z8r2k7v3xa9d';
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

function hostFor(hostId: string): Computer {
  const host = computers.find(item => item.hostId === hostId);
  if (host === undefined) {
    throw new Error(`Unknown host ${hostId}`);
  }
  return host;
}

function illustration() {
  return screen.getByTestId('session-ended-illustration', hidden);
}

async function renderBoard(
  Board: (props: BoardProps) => ReactElement,
  opts: {
    host: Computer;
    code: number;
    params?: BoardProps['params'];
    core?: ReturnType<typeof createFakeCore>;
    localDirectory?: LocalDirectorySeed;
  },
) {
  const core = opts.core ?? createFakeCore();
  const boardActions = actions();
  const host = opts.host;
  const view = await renderWithProviders(
    <Board
      actions={boardActions}
      code={opts.code}
      host={host}
      hostName={host.name}
      os={host.os}
      params={opts.params ?? { hostId: host.hostId }}
    />,
    { core, localDirectory: opts.localDirectory },
  );
  queryClients.push(view.queryClient);
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, actions: boardActions, core };
}

function lanHome(): Computer {
  const home = hostFor(HOME_PC);
  return {
    ...home,
    addresses: [
      ...home.addresses,
      {
        kind: 'lan',
        addr: '192.168.31.30',
        port: 7400,
        cidr: '192.168.31.0/24',
      },
    ],
  };
}

/** The paired Hosts on this phone, with `home` in place of the home PC. */
function withHome(home: Computer): LocalDirectorySeed {
  return {
    computers: computers.map(computer =>
      computer.hostId === HOME_PC ? home : computer,
    ),
  };
}

afterEach(async () => {
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
  jest.useRealTimers();
});

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(new Date(FIXTURE_NOW / 1000));
});

describe('OfflineBoard (110)', () => {
  it('renders the offline details and retries from the primary action', async () => {
    const { actions: boardActions } = await renderBoard(OfflineBoard, {
      host: hostFor(HOME_PC),
      code: 110,
      localDirectory: withHome(hostFor(HOME_PC)),
    });

    expect(screen.getByText('家里台式机离线了')).toBeOnTheScreen();
    expect(screen.getByText(/等它开机联网后再重试/)).toBeOnTheScreen();
    expect(screen.getByText('重试')).toBeEnabled();
    expect(screen.queryByText(/唤醒/, hidden)).toBeNull();
    // V34: nothing on its network to connect through, so no button.
    expect(screen.queryByText('查看同网段设备')).toBeNull();
    expect(screen.getByText('返回')).toBeOnTheScreen();
    expect(
      within(illustration()).getByText('Windows 11 · 2 小时前在线', hidden),
    ).toBeOnTheScreen();

    await fireEvent.press(screen.getByText('重试'));
    expect(boardActions.retry).toHaveBeenCalledTimes(1);
    expect(boardActions.retry).toHaveBeenCalledWith();
  });

  it('ignores a wake hint left on the computer and keeps retry', async () => {
    const host: Computer = {
      ...lanHome(),
      wake: { available: true, reason: null, relayName: '办公室 PC' },
    };
    const { actions: boardActions } = await renderBoard(OfflineBoard, {
      host,
      code: 110,
      localDirectory: withHome(host),
    });

    expect(screen.queryByText(/唤醒/, hidden)).toBeNull();
    expect(screen.queryByText('办公室 PC', hidden)).toBeNull();
    await fireEvent.press(screen.getByText('重试'));
    expect(boardActions.retry).toHaveBeenCalledTimes(1);
  });

  it('opens a same-LAN relay from the dialog', async () => {
    const { actions: boardActions } = await renderBoard(OfflineBoard, {
      host: lanHome(),
      code: 110,
      localDirectory: withHome(lanHome()),
    });

    expect(screen.getByText('查看同网段设备')).toBeEnabled();
    await fireEvent.press(screen.getByText('查看同网段设备'));
    const dialog = screen.getByTestId('relay-dialog-card');
    expect(within(dialog).getByText('同网段里在线的设备')).toBeOnTheScreen();
    await fireEvent.press(within(dialog).getByText('连接 Studio Mac mini'));

    expect(boardActions.openSession).toHaveBeenCalledWith(STUDIO);
  });
});

describe('NoHostBoard (120)', () => {
  function buttonLabels(): string[] {
    return screen
      .getAllByRole('button')
      .map(button => button.props.accessibilityLabel as string);
  }

  it('connects with system screen sharing first and offers retry second', async () => {
    const { actions: boardActions } = await renderBoard(NoHostBoard, {
      host: hostFor(MACBOOK),
      code: 120,
    });

    expect(
      screen.getByText('小明的 MacBook Pro 在线，但没有运行 Host'),
    ).toBeOnTheScreen();
    const labels = buttonLabels();
    expect(labels.indexOf('用屏幕共享连接')).toBeGreaterThanOrEqual(0);
    expect(labels.indexOf('用屏幕共享连接')).toBeLessThan(
      labels.indexOf('已装好，重试'),
    );
    expect(screen.queryByText('用系统屏幕共享')).toBeNull();

    await fireEvent.press(screen.getByText('用屏幕共享连接'));
    expect(boardActions.openScreenSharing).toHaveBeenCalledTimes(1);
    expect(boardActions.openTerminal).not.toHaveBeenCalled();

    await fireEvent.press(screen.getByText('已装好，重试'));
    expect(boardActions.retry).toHaveBeenCalledTimes(1);
    await fireEvent.press(screen.getByText('返回'));
    expect(boardActions.back).toHaveBeenCalledTimes(1);
  });

  it('names Remote Desktop as the primary action for a Windows computer', async () => {
    const { actions: boardActions } = await renderBoard(NoHostBoard, {
      host: hostFor(HOME_PC),
      code: 120,
    });

    const labels = buttonLabels();
    expect(labels.indexOf('用远程桌面连接')).toBeGreaterThanOrEqual(0);
    expect(labels.indexOf('用远程桌面连接')).toBeLessThan(
      labels.indexOf('已装好，重试'),
    );
    expect(screen.queryByText('改用远程桌面')).toBeNull();
    await fireEvent.press(screen.getByText('用远程桌面连接'));
    expect(boardActions.openScreenSharing).toHaveBeenCalledTimes(1);
  });

  it('names both system tools when the computer is not in the directory', async () => {
    const boardActions = actions();
    const view = await renderWithProviders(
      <NoHostBoard
        actions={boardActions}
        code={120}
        host={undefined}
        hostName="unknown-node"
        os={null}
        params={{ hostId: 'unknown-host', code: 120 }}
      />,
    );
    queryClients.push(view.queryClient);
    await act(async () => {
      await jest.advanceTimersByTimeAsync(1);
    });

    expect(
      screen.getByText(/不装 Host 也能用电脑自带的屏幕共享或远程桌面直接连/),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByText('用屏幕共享 / 远程桌面连接'));
    expect(boardActions.openScreenSharing).toHaveBeenCalledTimes(1);
  });

  it('offers no SSH install that needs a Hub install address', async () => {
    const { actions: boardActions } = await renderBoard(NoHostBoard, {
      host: hostFor(MACBOOK),
      code: 120,
    });

    expect(screen.queryByText(/SSH/, hidden)).toBeNull();
    expect(screen.queryByText(/Hub/, hidden)).toBeNull();
    expect(
      screen.queryByTestId('session-ended-illustration', hidden),
    ).toBeNull();
    expect(boardActions.openTerminal).not.toHaveBeenCalled();
    expect(screen.getByText('用屏幕共享连接')).toBeEnabled();
    expect(screen.getByText('已装好，重试')).toBeEnabled();
  });
});

describe('FingerprintBoard (150)', () => {
  const otherFp = Buffer.from(
    Array.from({ length: 32 }, (_, index) => 255 - index),
  ).toString('base64url');

  it('shows both fingerprints and cancels', async () => {
    const { actions: boardActions } = await renderBoard(FingerprintBoard, {
      host: hostFor(STUDIO),
      code: 150,
      params: {
        hostId: STUDIO,
        detail: {
          fp: otherFp,
          addr: '100.64.0.7',
          path: 'tailnet',
          host_version: '0.4.0',
        },
      },
    });

    const displayed = within(illustration()).getAllByText(/^SHA256 /, hidden);
    expect(displayed).toHaveLength(2);
    expect(displayed[0]?.props.children).not.toBe(displayed[1]?.props.children);
    for (const item of displayed) {
      expect(item.props.children).toMatch(/^SHA256 /);
    }
    await fireEvent.press(screen.getByText('取消'));
    expect(boardActions.back).toHaveBeenCalledTimes(1);
  });

  it('forgets the old fingerprint and starts pairing after confirmation', async () => {
    const core = createFakeCore();
    const { actions: boardActions } = await renderBoard(FingerprintBoard, {
      host: hostFor(STUDIO),
      code: 150,
      params: { hostId: STUDIO, detail: { fp: otherFp } },
      core,
    });

    await fireEvent.press(screen.getByText('我重装过，重新配对'));
    expect(screen.getByText('确定是你重装的吗？')).toBeOnTheScreen();
    const dialog = screen.getByTestId('fingerprint-dialog-card');
    await fireEvent.press(within(dialog).getByText('删掉旧指纹并配对'));

    await waitFor(() => {
      expect(core.calls).toContainEqual({
        method: 'hosts.forget',
        args: [STUDIO],
      });
    });
    await waitFor(() => {
      expect(boardActions.pair).toHaveBeenCalledWith('scan');
    });
  });

  it('holds the confirm dialog while forgetting (V57)', async () => {
    const core = createFakeCore();
    jest
      .spyOn(core.hosts, 'forget')
      .mockReturnValue(new Promise<void>(() => undefined));
    const { actions: boardActions } = await renderBoard(FingerprintBoard, {
      host: hostFor(STUDIO),
      code: 150,
      params: { hostId: STUDIO, detail: { fp: otherFp } },
      core,
    });

    await fireEvent.press(screen.getByText('我重装过，重新配对'));
    await fireEvent.press(screen.getByTestId('fingerprint-forget'));
    expect(screen.getByTestId('fingerprint-forget')).toBeBusy();
    await fireEvent.press(screen.getByTestId('fingerprint-forget'));
    expect(core.hosts.forget).toHaveBeenCalledTimes(1);
    expect(boardActions.pair).not.toHaveBeenCalled();
  });

  it('names the Host without the system when it is unknown (V60)', async () => {
    const boardActions = actions();
    const host = hostFor(STUDIO);
    const view = await renderWithProviders(
      <FingerprintBoard
        actions={boardActions}
        code={150}
        host={undefined}
        hostName="这台电脑"
        os={null}
        params={{ hostId: host.hostId, detail: { fp: otherFp } }}
      />,
    );
    queryClients.push(view.queryClient);
    expect(
      within(illustration()).getByText(
        '没重装过就别继续：到那台电脑上看看 Host 还在不在',
        hidden,
      ),
    ).toBeOnTheScreen();
    expect(screen.queryByText(/任务栏|菜单栏|tailnet/, hidden)).toBeNull();
  });

  it('reports a failed forget without starting pairing', async () => {
    const core = createFakeCore();
    core.failNext('hosts.forget', new Error('forget failed'));
    const { actions: boardActions } = await renderBoard(FingerprintBoard, {
      host: hostFor(STUDIO),
      code: 150,
      params: { hostId: STUDIO, detail: { fp: otherFp } },
      core,
    });

    await fireEvent.press(screen.getByText('我重装过，重新配对'));
    const dialog = screen.getByTestId('fingerprint-dialog-card');
    await fireEvent.press(within(dialog).getByText('删掉旧指纹并配对'));

    await waitFor(() => {
      expect(boardActions.toast).toHaveBeenCalledWith(
        '没删掉旧指纹：forget failed',
      );
    });
    expect(boardActions.pair).not.toHaveBeenCalled();
  });
});
