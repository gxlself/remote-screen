import { act, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FakeIpc } from '../../ipc/fake';
import { flush, renderPage } from '../../ipc/testing';
import type { DeviceSummary } from '../../ipc/types';
import { FakePlatform, SYSTEM_SETTINGS_URLS } from '../../platform';

import SetupPage from '.';

const DEVICE: DeviceSummary = {
  device_id: 'd-h17p-test',
  name: '测试 iPhone',
  permission: 'control',
  paths: ['tailnet'],
  last_connected_at_us: null,
};

function renderSetup(ipc: FakeIpc, platform = new FakePlatform()) {
  return renderPage(<SetupPage route={{ page: 'setup', query: {} }} />, { ipc, platform });
}

function renderWindowsSetup(ipc: FakeIpc) {
  return renderSetup(ipc, new FakePlatform({ os: 'windows' }));
}

function setupCalls(ipc: FakeIpc) {
  return ipc.calls.filter((call) => call.method === 'settings.set');
}

function sidebarButtons(): HTMLElement[] {
  const list = screen.getByRole('list', { name: '设置步骤' });
  return within(list).getAllByRole('button');
}

function sidebarButton(index: number): HTMLElement {
  const button = sidebarButtons()[index];
  if (button === undefined) {
    throw new Error(`Missing setup step button ${index}`);
  }
  return button;
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('SetupPage', () => {
  it('renders the permission steps when status permissions are null', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    ipc.state.status.permissions = null as unknown as Record<string, unknown>;
    renderSetup(ipc);
    await flush();

    expect(screen.getByRole('heading', { name: '允许 Remote Host 录制屏幕' })).toBeInTheDocument();
    expect(screen.getByText('等待授权')).toBeInTheDocument();
  });

  it('starts at the first incomplete step and shows tailnet state', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    renderSetup(ipc);
    await flush();

    expect(screen.getByText('第 2 步，共 4 步')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '允许 Remote Host 录制屏幕' })).toBeInTheDocument();

    expect(sidebarButton(1)).toHaveAttribute('aria-current', 'step');
    expect(sidebarButton(0)).toHaveTextContent('已连接 · 100.64.0.7');
    expect(sidebarButton(1)).toHaveTextContent('等待授权');
  });

  it('opens the screen recording system settings page', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    const { platform, user } = renderSetup(ipc);
    await flush();

    await user.click(screen.getByRole('button', { name: '打开系统设置' }));

    expect(platform.calls).toContainEqual({
      op: 'openUrl',
      args: [SYSTEM_SETTINGS_URLS.screenRecording],
    });
  });

  it('advances after permissionsChanged and opens accessibility settings', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    const { platform, user } = renderSetup(ipc);
    await flush();

    ipc.state.status.permissions.screen_recording = true;
    act(() => {
      ipc.emit('event.permissionsChanged', {});
    });
    await flush();

    expect(
      screen.getByRole('heading', { name: '允许 Remote Host 控制鼠标和键盘' }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '打开系统设置' }));
    expect(platform.calls).toContainEqual({
      op: 'openUrl',
      args: [SYSTEM_SETTINGS_URLS.accessibility],
    });
  });

  it('advances after polling and opens the pairing window', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    ipc.state.status.permissions.screen_recording = true;
    const { platform, user } = renderSetup(ipc);
    await flush();

    expect(
      screen.getByRole('heading', { name: '允许 Remote Host 控制鼠标和键盘' }),
    ).toBeInTheDocument();
    ipc.state.status.permissions.accessibility = true;

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    await flush();

    expect(screen.getByRole('heading', { name: '配对你的手机' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '显示配对二维码' }));
    expect(platform.calls).toContainEqual({
      op: 'showWindow',
      args: ['pair', 'pair'],
    });
  });

  it('finishes after a device is paired and records the completion flag', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    ipc.state.status.permissions.screen_recording = true;
    ipc.state.status.permissions.accessibility = true;
    const { platform, user } = renderSetup(ipc);
    await flush();

    expect(screen.getByRole('heading', { name: '配对你的手机' })).toBeInTheDocument();
    ipc.state.devices.push(DEVICE);
    act(() => {
      ipc.emit('event.pairingResult', {
        status: 1,
        host_id: 'h-test',
        host_name: 'studio-mac',
        attempts_left: 0,
        device_fp: 'fp-test',
      });
    });
    await flush();

    expect(screen.getByRole('heading', { name: '全部设置好了' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '完成' }));

    expect(localStorage.getItem('rs.setup.done')).toBe('1');
    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['setup'] });
  });

  it('shows the completion page immediately when everything is done', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    renderSetup(ipc);
    await flush();

    expect(screen.getByRole('heading', { name: '全部设置好了' })).toBeInTheDocument();
    expect(sidebarButtons()).toHaveLength(4);
  });

  it('shows the tailnet step and disables the back button when disconnected from tailnet', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = [];
    renderSetup(ipc);
    await flush();

    expect(screen.getByRole('heading', { name: '先把这台 Mac 连上 tailnet' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '上一步' })).toBeDisabled();
    expect(sidebarButton(0)).toHaveTextContent('还没连上');
  });

  it('lets the user revisit and continue from a completed step', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    ipc.state.status.permissions.screen_recording = true;
    const { user } = renderSetup(ipc);
    await flush();

    await user.click(sidebarButton(1));
    expect(screen.getByRole('heading', { name: '允许 Remote Host 录制屏幕' })).toBeInTheDocument();
    expect(screen.getByText('这一步已经完成')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '下一步' }));
    expect(
      screen.getByRole('heading', { name: '允许 Remote Host 控制鼠标和键盘' }),
    ).toBeInTheDocument();
  });

  it('hides without writing the completion flag when choosing later', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    const { platform, user } = renderSetup(ipc);
    await flush();

    await user.click(screen.getByRole('button', { name: '稍后' }));

    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['setup'] });
    expect(localStorage.getItem('rs.setup.done')).toBeNull();
  });

  it('shows the agent-down banner while keeping the last known steps', async () => {
    const ipc = FakeIpc.fromScenario('offline');
    renderSetup(ipc);
    await flush();

    expect(screen.getByText('Remote Host 服务没有运行')).toBeInTheDocument();
  });
});

describe('SetupPage on Windows', () => {
  function windowsSetup(): FakeIpc {
    const ipc = FakeIpc.fromScenario('setup');
    ipc.state.status.tailnet_addresses = ['100.64.0.7'];
    ipc.state.settings.intranet.enabled = false;
    ipc.state.nics = [
      { name: '以太网', addresses: ['10.30.12.8'], networks: [], tailnet: false },
      { name: 'Wi-Fi', addresses: [], networks: [], tailnet: false },
    ];
    return ipc;
  }

  async function reachLanStep(ipc = windowsSetup()) {
    const rendered = renderWindowsSetup(ipc);
    await flush();
    return { ...rendered, ipc };
  }

  it('shows the Windows steps and starts on the first incomplete step', async () => {
    const ipc = windowsSetup();
    ipc.state.status.tailnet_addresses = [];
    renderWindowsSetup(ipc);
    await flush();

    expect(screen.getByText('第 1 步，共 4 步')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '先把这台电脑连上 tailnet' })).toBeInTheDocument();
    expect(sidebarButtons()).toHaveLength(4);
    expect(sidebarButton(0)).toHaveTextContent('连接 tailnet');
    expect(sidebarButton(1)).toHaveTextContent('系统服务');
    expect(sidebarButton(2)).toHaveTextContent('内网连接');
    expect(sidebarButton(3)).toHaveTextContent('配对手机');
  });

  it('stops on the intranet step after tailnet and service are ready', async () => {
    await reachLanStep();

    expect(screen.getByText('第 3 步，共 4 步')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '要不要允许从内网连进来？' })).toBeInTheDocument();
    expect(sidebarButton(0)).toHaveTextContent('100.64.0.7');
    expect(sidebarButton(1)).toHaveTextContent('已运行 · 锁屏时也能操作');
  });

  it('skips the intranet step without saving settings', async () => {
    const { user, ipc } = await reachLanStep();

    expect(sidebarButton(2)).toHaveTextContent('默认关闭，可跳过');
    await user.click(screen.getByRole('button', { name: '跳过，以后再说' }));

    expect(screen.getByRole('heading', { name: '配对你的手机' })).toBeInTheDocument();
    expect(sidebarButton(2)).toHaveTextContent('已跳过 · 只走 tailnet');
    expect(setupCalls(ipc)).toHaveLength(0);
  });

  it('validates and saves the intranet form', async () => {
    const { user, ipc } = await reachLanStep();

    await user.click(screen.getByRole('button', { name: '打开内网连接' }));
    expect(screen.getByRole('heading', { name: '内网连接已打开，选一下范围' })).toBeInTheDocument();
    expect(sidebarButton(2)).toHaveTextContent('已打开 · 待保存');
    expect(screen.getByRole('button', { name: '保存并继续' })).toBeDisabled();

    await user.click(screen.getByRole('checkbox', { name: '以太网' }));
    await user.type(screen.getByRole('textbox', { name: '新的来源网段' }), '10.99.0.0/16');
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByRole('button', { name: '保存并继续' })).toBeEnabled();

    await user.type(screen.getByRole('textbox', { name: '新的来源网段' }), '10.99.0.0/33');
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByRole('alert')).toHaveTextContent('格式不对，要写成 10.30.0.0/16 这样');

    await user.click(screen.getByRole('button', { name: '保存并继续' }));
    await flush();

    expect(setupCalls(ipc)).toEqual([
      {
        method: 'settings.set',
        params: {
          intranet: {
            enabled: true,
            interfaces: ['以太网'],
            allow_sources: ['10.99.0.0/16'],
          },
        },
      },
    ]);
    expect(screen.getByRole('heading', { name: '配对你的手机' })).toBeInTheDocument();
    expect(sidebarButton(2)).toHaveTextContent('已打开 · 以太网');
  });

  it('keeps the intranet form open and reports a failed save', async () => {
    const { user, ipc } = await reachLanStep();
    await user.click(screen.getByRole('button', { name: '打开内网连接' }));
    await user.click(screen.getByRole('checkbox', { name: '以太网' }));
    await user.type(screen.getByRole('textbox', { name: '新的来源网段' }), '10.99.0.0/16');
    await user.click(screen.getByRole('button', { name: '添加' }));
    ipc.failNext('settings.set', { code: -32603, message: 'disk full' });

    await user.click(screen.getByRole('button', { name: '保存并继续' }));
    await flush();

    expect(screen.getByRole('alert')).toHaveTextContent('设置没有保存：disk full');
    expect(screen.getByRole('heading', { name: '内网连接已打开，选一下范围' })).toBeInTheDocument();
  });

  it('reopens an already saved intranet form with its current values', async () => {
    const ipc = windowsSetup();
    ipc.state.settings.intranet = {
      ...ipc.state.settings.intranet,
      enabled: true,
      interfaces: ['以太网'],
      allow_sources: ['10.99.0.0/16'],
    };
    const { user } = await reachLanStep(ipc);

    expect(screen.getByRole('heading', { name: '配对你的手机' })).toBeInTheDocument();
    await user.click(sidebarButton(2));
    await user.click(screen.getByRole('button', { name: '修改' }));
    await flush();

    expect(screen.getByRole('heading', { name: '内网连接已打开，选一下范围' })).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: '以太网' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByText('10.99.0.0/16')).toBeInTheDocument();
  });

  it('describes intranet pairing and can finish later', async () => {
    const { user, platform } = await reachLanStep();
    await user.click(screen.getByRole('button', { name: '打开内网连接' }));
    await user.click(screen.getByRole('checkbox', { name: '以太网' }));
    await user.type(screen.getByRole('textbox', { name: '新的来源网段' }), '10.99.0.0/16');
    await user.click(screen.getByRole('button', { name: '添加' }));
    await user.click(screen.getByRole('button', { name: '保存并继续' }));
    await flush();

    expect(screen.getByText('在 tailnet 上或从公司 VPN 来都能配对。')).toBeInTheDocument();
    expect(screen.getByText(/用 Windows 账户密码配对/, { selector: 'span' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '显示配对二维码' }));
    expect(platform.calls).toContainEqual({ op: 'showWindow', args: ['pair', 'pair'] });
    await user.click(screen.getByRole('button', { name: '稍后配对' }));

    expect(screen.getByRole('heading', { name: '全部设置好了' })).toBeInTheDocument();
    expect(screen.getByText('以后在托盘点 Remote Host 图标就能看到谁在连接。')).toBeInTheDocument();
  });

  it('shows the service as stopped when status.get fails', async () => {
    const ipc = windowsSetup();
    ipc.failNext('status.get', { code: -32603, message: 'unavailable' });
    const { user } = renderWindowsSetup(ipc);
    await flush();

    await user.click(sidebarButton(1));
    expect(screen.getByRole('alert')).toHaveTextContent('服务没有运行');
    expect(
      screen.getByText('重新安装 Remote Host 或在服务管理器里启动 RemoteHostService'),
    ).toBeInTheDocument();
    expect(sidebarButton(1)).toHaveTextContent('服务没有运行');
  });

  it('does not expose Mac-only setup wording', async () => {
    const ipc = windowsSetup();
    ipc.state.status.tailnet_addresses = [];
    const { container } = renderWindowsSetup(ipc);
    await flush();

    expect(container.textContent ?? '').not.toMatch(/Mac|macOS|辅助功能|屏幕录制/);
  });
});
