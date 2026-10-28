import { act, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { flush, renderPage } from '../../ipc/testing';
import { FakeIpc } from '../../ipc/fake';

import UpdatePage from '.';

const NOW_MS = 1_792_465_134_000;

function createIpc(scenario: string | null = 'update'): FakeIpc {
  return FakeIpc.fromScenario(scenario, { now: () => NOW_MS });
}

function page(scenario: string | null = 'update') {
  const ipc = createIpc(scenario);
  const result = renderPage(<UpdatePage route={{ page: 'update', query: {} }} />, { ipc });
  return { ...result, ipc };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW_MS);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('UpdatePage', () => {
  it('shows the pending release, checked download and notes', async () => {
    page();
    await flush();

    expect(screen.getByRole('heading', { name: 'Remote Host 有新版本' })).toBeInTheDocument();
    expect(screen.getByText('0.3.1')).toBeInTheDocument();
    expect(screen.getByText('0.3.2')).toBeInTheDocument();
    expect(screen.getAllByText('稳定版')).toHaveLength(2);
    expect(screen.getByText('已下载 · 签名已校验')).toBeInTheDocument();
    expect(screen.getByText('小明的 iPhone 正在控制这台 Mac')).toBeInTheDocument();
    expect(screen.getByText('会话结束后自动安装，不会打断你')).toBeInTheDocument();
    expect(screen.getByText('42:18')).toBeInTheDocument();
    expect(screen.getByText('修复')).toBeInTheDocument();
    expect(screen.getByText('新增')).toBeInTheDocument();
    expect(screen.getByText('TCP 模式下偶发花屏')).toBeInTheDocument();
    expect(screen.getByText('「内网连接每次确认」')).toBeInTheDocument();
  });

  it('keeps the session when choosing to install afterwards', async () => {
    const { ipc, platform, user } = page();
    await flush();

    expect(screen.getByRole('button', { name: '现在安装' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '好，结束后安装' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '好，结束后安装' }));

    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['update'] });
    expect(ipc.calls.some((call) => call.method === 'update.installNow')).toBe(false);
  });

  it('confirms before disconnecting and installing', async () => {
    const { ipc, user } = page();
    await flush();

    await user.click(screen.getByRole('button', { name: '现在安装' }));
    expect(screen.getByText('现在安装会断开小明的 iPhone的会话。')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '断开并现在安装' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '取消' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '断开并现在安装' }));
    await flush();

    expect(ipc.calls).toContainEqual({ method: 'update.installNow', params: null });

    act(() => {
      ipc.state.update.state = 'installing';
      ipc.emit('event.updateState', { state: 'installing' });
    });
    await flush();

    expect(screen.getByText('正在安装…')).toBeInTheDocument();
    for (const button of screen.getAllByRole('button')) {
      expect(button).toBeDisabled();
    }
  });

  it('saves auto-install changes optimistically and reflects cancellation', async () => {
    const { ipc, user } = page();
    await flush();

    await user.click(screen.getByRole('checkbox', { name: /会话结束后安装/ }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'settings.set',
      params: { update: { auto_install: false } },
    });
    expect(screen.getByText('已取消「会话结束后安装」，需要你自己选时间')).toBeInTheDocument();
    expect(screen.getByText('现在安装会断开小明的 iPhone的会话。')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '稍后提醒我' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '断开并现在安装' })).toBeInTheDocument();
  });

  it('switches channel and reloads settings', async () => {
    const { ipc, user } = page();
    await flush();
    const before = ipc.calls.filter((call) => call.method === 'settings.get').length;

    await user.click(screen.getByRole('radio', { name: '测试版' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'update.setChannel',
      params: { channel: 'beta' },
    });
    expect(screen.getByText('更早拿到新功能，偶尔会有小问题')).toBeInTheDocument();
    expect(ipc.calls.filter((call) => call.method === 'settings.get')).toHaveLength(before + 1);
  });

  it('installs immediately when there are no sessions', async () => {
    const { ipc, user } = page('update-ready');
    await flush();

    expect(screen.getByText('现在没有会话，可以马上安装')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '稍后' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '现在安装' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '现在安装' }));
    await flush();
    expect(ipc.calls).toContainEqual({ method: 'update.installNow', params: null });
  });

  it('updates the banner when the last session ends', async () => {
    const { ipc } = page();
    await flush();

    await act(async () => {
      await ipc.call('sessions.disconnect', { session_id: 's-1' });
    });
    await flush();

    expect(screen.getByText('现在没有会话，可以马上安装')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '稍后' })).toBeInTheDocument();
  });

  it('shows the latest state without an update', async () => {
    page('idle');
    await flush();

    expect(screen.getByText('Remote Host 已是最新版本')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '关闭' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '现在安装' })).not.toBeInTheDocument();
  });

  it('reports a failed update', async () => {
    const ipc = createIpc();
    ipc.state.update = { state: 'failed', version: '0.3.2', notes: null };
    renderPage(<UpdatePage route={{ page: 'update', query: {} }} />, { ipc });
    await flush();

    expect(screen.getByRole('alert')).toHaveTextContent('这次更新没有装上，会自动再试');
  });

  it('handles missing notes and plain note lines', async () => {
    const missing = createIpc();
    missing.state.update = { state: 'idle', version: '0.3.2', notes: null };
    const first = renderPage(<UpdatePage route={{ page: 'update', query: {} }} />, {
      ipc: missing,
    });
    await flush();
    expect(screen.getByText('这个版本没有写更新说明')).toBeInTheDocument();
    first.unmount();

    const plain = createIpc();
    plain.state.update = { state: 'idle', version: '0.3.2', notes: '修复：A\n\n其他' };
    renderPage(<UpdatePage route={{ page: 'update', query: {} }} />, { ipc: plain });
    await flush();

    expect(screen.getByText('A')).toBeInTheDocument();
    expect(screen.getByText('其他')).toBeInTheDocument();
    expect(screen.getAllByText('修复')).toHaveLength(1);
  });

  it('alerts when installNow cannot start', async () => {
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => undefined);
    const { ipc, user } = page();
    ipc.failNext('update.installNow', { code: -32000, message: 'installer unavailable' });
    await flush();

    await user.click(screen.getByRole('button', { name: '现在安装' }));
    await user.click(screen.getByRole('button', { name: '断开并现在安装' }));
    await flush();
    await waitFor(() => {
      expect(alert).toHaveBeenCalledWith('没有开始安装：installer unavailable');
    });
  });
});
