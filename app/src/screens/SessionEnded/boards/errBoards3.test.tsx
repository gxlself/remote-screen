import {
  act,
  cleanup,
  fireEvent,
  screen,
} from '@testing-library/react-native';
import VersionBoard from './VersionBoard';
import LoginWindowBoard from './LoginWindowBoard';
import type { BoardProps, EndActions } from '../boardTypes';
import { createFakeCore } from '../../../native/rscore/fakeCore';
import { FakePlatform } from '../../../native/platform/fakePlatform';
import { computers } from '../../../native/rscore/fixtures';
import type { Computer, NetworkStatus } from '../../../native/rscore/types';
import { renderWithProviders } from '../../../test/render';

const HOME = 'd2n6c4v8b1x5za7e';
const STUDIO = 'h7v2m9q3k8x4tb1a';
const hidden = { includeHiddenElements: true } as const;

function home(): Computer {
  const found = computers.find(computer => computer.hostId === HOME);
  if (found === undefined) {
    throw new Error('home fixture missing');
  }
  return found;
}

function studio(): Computer {
  const found = computers.find(computer => computer.hostId === STUDIO);
  if (found === undefined) {
    throw new Error('studio fixture missing');
  }
  return found;
}

function actions(): EndActions & { [K in keyof EndActions]: jest.Mock } {
  const retry = jest.fn();
  return {
    retry,
    openSession: jest.fn(),
    back: jest.fn(),
    openTerminal: jest.fn(),
    openScreenSharing: jest.fn(async () => undefined),
    copyText: jest.fn(),
    copyCode: jest.fn(),
    pair: jest.fn(),
    openDiagnostics: jest.fn(),
    toast: jest.fn(),
  };
}

type RenderOptions = {
  code: number;
  host: Computer;
  actionBag?: ReturnType<typeof actions>;
  platform?: FakePlatform;
  network?: NetworkStatus;
};

async function renderBoard({
  code,
  host,
  actionBag = actions(),
  platform = new FakePlatform(),
  network,
}: RenderOptions) {
  const core = createFakeCore(network === undefined ? {} : { network });
  const Board = code === 170 || code === 171 ? VersionBoard : LoginWindowBoard;
  const props: BoardProps = {
    params: { hostId: host.hostId, code },
    code,
    host,
    hostName: host.name,
    os: host.os,
    actions: actionBag,
  };
  const rendered = await renderWithProviders(<Board {...props} />, {
    core,
    platform,
  });
  return { ...rendered, core, platform, actions: actionBag };
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
  jest.restoreAllMocks();
});

describe('E170 VersionBoard', () => {
  it('shows the versions and asks to update the host, then retry', async () => {
    const bag = actions();
    await renderBoard({ code: 170, host: home(), actionBag: bag });
    await advance(1);

    expect(
      screen.getByText('家里台式机上的 Host 太旧，连不上'),
    ).toBeOnTheScreen();
    expect(
      screen.getByText(/到那台电脑上更新 Host，装好后回来重试/),
    ).toBeOnTheScreen();
    // V59: no protocol name in the meta line.
    expect(screen.queryByText(/RSP/, hidden)).toBeNull();
    expect(screen.getByText('0.2.4', hidden)).toBeOnTheScreen();
    expect(screen.getByText('≥ 0.3.0', hidden)).toBeOnTheScreen();
    expect(screen.queryByRole('button', { name: '远程升级' })).toBeNull();

    await fireEvent.press(screen.getByRole('button', { name: '已更新，重试' }));
    expect(bag.retry).toHaveBeenCalledTimes(1);
    expect(bag.retry).toHaveBeenCalledWith();
    await fireEvent.press(screen.getByRole('button', { name: '返回' }));
    expect(bag.back).toHaveBeenCalledTimes(1);
  });

  it('offers no remote upgrade even when an update is listed', async () => {
    const online: Computer = {
      ...home(),
      online: true,
      hostRunning: true,
      update: { channel: 'stable', available: '0.3.1' },
    };
    const { core } = await renderBoard({ code: 170, host: online });
    await advance(1);

    expect(screen.queryByRole('button', { name: '远程升级' })).toBeNull();
    expect(screen.queryByText(/远程升级|可以升到|Hub/, hidden)).toBeNull();
    expect(
      screen.getByRole('button', { name: '已更新，重试' }),
    ).toBeOnTheScreen();
    expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual(
      [],
    );
  });

  it('can switch to the desktop fallback from the old state', async () => {
    const bag = actions();
    await renderBoard({ code: 170, host: home(), actionBag: bag });
    await fireEvent.press(screen.getByRole('button', { name: '改用远程桌面' }));
    expect(bag.openScreenSharing).toHaveBeenCalledTimes(1);
  });
});

describe('E171 VersionBoard', () => {
  it('opens this app in the App Store', async () => {
    const platform = new FakePlatform();
    await renderBoard({ code: 171, host: studio(), platform });
    await advance(1);

    expect(
      screen.getByText('这个 App 太旧，Studio Mac mini 不接受'),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '去更新 App' }));
    expect(platform.opened).toEqual([
      'itms-apps://apps.apple.com/app/id6826963518',
    ]);
  });
});

describe('E190 LoginWindowBoard', () => {
  it('hands login to system screen sharing and retries by hand', async () => {
    const bag = actions();
    const loginHost: Computer = {
      ...studio(),
      console: { loggedIn: false, locked: false, loginWindow: true },
    };
    await renderBoard({ code: 190, host: loginHost, actionBag: bag });
    await advance(1);

    expect(
      screen.getByText('Studio Mac mini 重启了，还没登录'),
    ).toBeOnTheScreen();
    expect(
      screen.getByText('示意图 · 登录前没有实时画面', hidden),
    ).toBeOnTheScreen();
    expect(screen.queryByText(/alice|XL/, hidden)).toBeNull();
    // V50: the built-in screen sharing opens first, so no "system".
    expect(screen.queryByText(/系统屏幕共享|tailnet/, hidden)).toBeNull();
    await fireEvent.press(
      screen.getByRole('button', { name: '用屏幕共享登录' }),
    );
    expect(bag.openScreenSharing).toHaveBeenCalledTimes(1);

    await advance(10000);
    expect(bag.retry).not.toHaveBeenCalled();
    await fireEvent.press(screen.getByRole('button', { name: '已登录，重试' }));
    expect(bag.retry).toHaveBeenCalledTimes(1);
    expect(bag.retry).toHaveBeenCalledWith();
  });

  it('uses the Windows wording for a Windows host', async () => {
    const office = computers.find(
      computer => computer.hostId === 'w3k8p2x7m4q9tb6c',
    );
    if (office === undefined) {
      throw new Error('office fixture missing');
    }
    await renderBoard({ code: 190, host: office });
    await advance(1);
    expect(screen.getByText('电脑停在登录界面')).toBeOnTheScreen();
    expect(
      screen.getByRole('button', { name: '已登录，重试' }),
    ).toBeOnTheScreen();
    expect(
      screen.getByRole('button', { name: '用远程桌面登录' }),
    ).toBeOnTheScreen();
  });

  it('names both tools when the system is unknown (V60)', async () => {
    const bag = actions();
    const view = await renderWithProviders(
      <LoginWindowBoard
        actions={bag}
        code={190}
        host={undefined}
        hostName="这台电脑"
        os={null}
        params={{ hostId: 'unknown-host', code: 190 }}
      />,
    );
    await advance(1);
    expect(screen.getByText('电脑停在登录界面')).toBeOnTheScreen();
    expect(screen.queryByText(/Mac/, hidden)).toBeNull();
    await fireEvent.press(
      screen.getByRole('button', { name: '用屏幕共享或远程桌面登录' }),
    );
    expect(bag.openScreenSharing).toHaveBeenCalledTimes(1);
    view.queryClient.clear();
  });
});

describe('E191 LoginWindowBoard', () => {
  it('offers SSH on the same LAN and copies the command', async () => {
    const bag = actions();
    const filevaultHost: Computer = {
      ...studio(),
      online: false,
      console: null,
    };
    await renderBoard({
      code: 191,
      host: filevaultHost,
      actionBag: bag,
      network: {
        ifaces: [{ name: 'en0', addrs: ['192.168.31.45/24'], isVpn: false }],
      },
    });
    await advance(1);

    expect(
      screen.getByText('通过 SSH 解锁要这台 Mac 开着远程登录'),
    ).toBeOnTheScreen();
    await fireEvent.press(
      screen.getByRole('button', { name: '通过 SSH 解锁磁盘' }),
    );
    expect(screen.getByText('ssh user@192.168.31.20')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '复制命令' }));
    expect(bag.copyText).toHaveBeenCalledWith(
      'ssh user@192.168.31.20',
      '已复制命令',
    );
  });

  it('hides SSH outside the host LAN, retries and goes back', async () => {
    const bag = actions();
    const other: Computer = {
      ...studio(),
      online: false,
      console: null,
    };
    await renderBoard({
      code: 191,
      host: other,
      actionBag: bag,
      network: {
        ifaces: [{ name: 'en0', addrs: ['10.99.4.27'], isVpn: false }],
      },
    });
    await advance(1);

    expect(
      screen.queryByRole('button', { name: '通过 SSH 解锁磁盘' }),
    ).toBeNull();
    // V31: no developer note and no SSH requirement without SSH.
    expect(screen.queryByText(/实测|远程登录/, hidden)).toBeNull();
    await fireEvent.press(screen.getByRole('button', { name: '已解锁，重试' }));
    expect(bag.retry).toHaveBeenCalledTimes(1);
    await fireEvent.press(
      screen.getByRole('button', { name: '我去 Mac 前解锁' }),
    );
    expect(bag.back).toHaveBeenCalledTimes(1);
  });
});
