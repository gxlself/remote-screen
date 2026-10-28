import { act, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { parseHash } from '../../app/router';
import { FakeIpc } from '../../ipc/fake';
import { flush, renderPage } from '../../ipc/testing';
import { FakePlatform } from '../../platform';

import MenuPage from './index';

const NOW = new Date('2026-10-21T09:30:00.000Z').getTime();
const MENU_ROUTE = parseHash('#/menu');

function renderMenu(scenario: string, platform = new FakePlatform()) {
  const ipc = FakeIpc.fromScenario(scenario);
  return { ...renderPage(<MenuPage route={MENU_ROUTE} />, { ipc, platform }), ipc };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('MenuPage', () => {
  it('shows host status and a controller session, and refreshes its elapsed time', async () => {
    renderMenu('controlled');
    await flush();

    expect(screen.getByText('Remote Host')).toBeInTheDocument();
    expect(screen.getByText('0.3.1')).toBeInTheDocument();
    expect(screen.getByText('在线')).toBeInTheDocument();
    expect(screen.getByText('100.64.0.7')).toBeInTheDocument();
    expect(screen.getByText('小明的 iPhone')).toBeInTheDocument();
    expect(screen.getByText('控制中')).toBeInTheDocument();
    expect(screen.getByText('已连接 42:18')).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });

    expect(screen.getByText('已连接 42:19')).toBeInTheDocument();
  });

  it('changes the controller to view only', async () => {
    const { ipc, user } = renderMenu('controlled');
    await flush();

    await user.click(screen.getByRole('button', { name: '改为仅观看' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'sessions.setViewOnly',
      params: { session_id: 's-1' },
    });
    expect(screen.getByText('观看中')).toBeInTheDocument();
    expect(screen.queryByText('控制中')).not.toBeInTheDocument();
  });

  it('disconnects the controller and hides the session area', async () => {
    const { ipc, user } = renderMenu('controlled');
    await flush();

    await user.click(screen.getByRole('button', { name: '断开' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'sessions.disconnect',
      params: { session_id: 's-1' },
    });
    expect(screen.queryByText('小明的 iPhone')).not.toBeInTheDocument();
    expect(screen.queryByText('控制中')).not.toBeInTheDocument();
  });

  it('renders handoff sessions and disconnects all of them in order', async () => {
    const { ipc, user } = renderMenu('handoff');
    await flush();

    expect(screen.getByText('正在连接 · 2 台')).toBeInTheDocument();
    expect(screen.getByText('小明的 iPhone')).toBeInTheDocument();
    expect(screen.getByText('小明的 iPad')).toBeInTheDocument();
    expect(screen.getByText('别人的设备连进来时，要正在控制的人或你同意；')).toBeInTheDocument();
    expect(screen.getByText('30 秒没人理，只给观看。')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '全部断开' }));
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'sessions.disconnect')).toEqual([
      { method: 'sessions.disconnect', params: { session_id: 's-1' } },
      { method: 'sessions.disconnect', params: { session_id: 's-2' } },
    ]);
  });

  it('optimistically saves a switch and rolls it back on failure', async () => {
    const { ipc, user } = renderMenu('controlled');
    await flush();

    const allowSwitch = screen.getByRole('switch', { name: '允许被远程控制' });
    expect(allowSwitch).toHaveAttribute('aria-checked', 'true');

    await user.click(allowSwitch);
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'settings.set',
      params: { access: { allow_remote_control: false } },
    });
    expect(allowSwitch).toHaveAttribute('aria-checked', 'false');

    ipc.failNext('settings.set', {
      code: -32000,
      message: 'disk full',
      data: { code: 'io' },
    });
    await user.click(allowSwitch);
    await flush();

    expect(allowSwitch).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByRole('alert')).toHaveTextContent('设置没有保存：disk full');
  });

  it('maps the other two switches to access settings', async () => {
    const { ipc, user } = renderMenu('controlled');
    await flush();

    await user.click(screen.getByRole('switch', { name: '新设备连接前先问我' }));
    await flush();
    await user.click(screen.getByRole('switch', { name: '会话结束后锁定屏幕' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'settings.set',
      params: { access: { attended_mode: true } },
    });
    expect(ipc.calls).toContainEqual({
      method: 'settings.set',
      params: { access: { lock_on_end: true } },
    });
  });

  it('routes granted permissions to permission settings and missing permissions to setup', async () => {
    const granted = renderMenu('controlled');
    await flush();

    expect(screen.getByText('全部已授权')).toBeInTheDocument();
    await granted.user.click(screen.getByRole('button', { name: /权限与设置/ }));
    await flush();

    expect(granted.platform.calls).toEqual([
      { op: 'showWindow', args: ['settings', 'settings/permissions'] },
      { op: 'hideWindow', args: ['menu'] },
    ]);

    granted.unmount();
    const missing = renderMenu('setup');
    await flush();

    expect(screen.getByText('缺少权限')).toBeInTheDocument();
    await missing.user.click(screen.getByRole('button', { name: /权限与设置/ }));
    await flush();

    expect(missing.platform.calls).toEqual([
      { op: 'showWindow', args: ['setup', 'setup'] },
      { op: 'hideWindow', args: ['menu'] },
    ]);
  });

  it('opens devices, pairing, and history windows', async () => {
    const { platform, user } = renderMenu('controlled');
    await flush();

    expect(screen.getByText('3 台')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /已配对的设备/ }));
    await flush();
    await user.click(screen.getByRole('button', { name: '显示配对二维码…' }));
    await flush();
    await user.click(screen.getByRole('button', { name: '连接记录…' }));
    await flush();

    expect(platform.calls).toEqual([
      { op: 'showWindow', args: ['settings', 'settings/devices'] },
      { op: 'hideWindow', args: ['menu'] },
      { op: 'showWindow', args: ['pair', 'pair'] },
      { op: 'hideWindow', args: ['menu'] },
      { op: 'showWindow', args: ['settings', 'settings/history'] },
      { op: 'hideWindow', args: ['menu'] },
    ]);
  });

  it('shows an update entry only when an update version is available', async () => {
    const update = renderMenu('update');
    await flush();

    expect(screen.getByText('有新版本 0.3.2…')).toBeInTheDocument();
    await update.user.click(screen.getByRole('button', { name: '有新版本 0.3.2…' }));
    await flush();

    expect(update.platform.calls).toEqual([
      { op: 'showWindow', args: ['update', 'update'] },
      { op: 'hideWindow', args: ['menu'] },
    ]);

    update.unmount();
    renderMenu('controlled');
    await flush();

    expect(screen.queryByText(/有新版本/)).not.toBeInTheDocument();
  });

  it('quits from the footer row', async () => {
    const { platform, user } = renderMenu('controlled');
    await flush();

    await user.click(screen.getByRole('button', { name: /退出 Remote Host/ }));
    await flush();

    expect(platform.calls).toContainEqual({ op: 'quit', args: [] });
  });

  it('shows the agent-down banner and disables controls while offline', async () => {
    renderMenu('offline');
    await flush();

    expect(screen.getByText('Remote Host 服务没有运行')).toBeInTheDocument();
    const switches = screen.getAllByRole('switch');
    expect(switches).toHaveLength(3);
    for (const item of switches) {
      expect(item).toBeDisabled();
    }
    for (const button of screen.getAllByRole('button')) {
      expect(button).toBeDisabled();
    }
  });

  it('hides the session area and disconnect-all row when idle', async () => {
    renderMenu('idle');
    await flush();

    expect(screen.queryByText('控制中')).not.toBeInTheDocument();
    expect(screen.queryByText('小明的 iPhone')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '全部断开' })).not.toBeInTheDocument();
  });

  it('hides the menu window on Escape', async () => {
    const { platform, user } = renderMenu('controlled');
    await flush();

    await user.keyboard('{Escape}');
    await flush();

    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['menu'] });
  });

  it('shows Windows service guidance, a combined settings entry, and the update action', async () => {
    const { platform, user } = renderMenu('update', new FakePlatform({ os: 'windows' }));
    await flush();

    expect(screen.getByText('锁屏、登录界面和管理员提示时也能操作')).toBeInTheDocument();
    expect(screen.getByText('由系统服务负责，切换桌面时画面不断')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^有新版本 0\.3\.2\s*更新$/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /设置与连接记录…/ })).toBeInTheDocument();
    expect(screen.queryByText('权限与设置…')).not.toBeInTheDocument();
    expect(screen.queryByText('连接记录…')).not.toBeInTheDocument();
    expect(screen.queryByText('⌘Q')).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /设置与连接记录…/ }));
    await flush();

    expect(platform.calls).toContainEqual({
      op: 'showWindow',
      args: ['settings', 'settings/general'],
    });
  });
});
