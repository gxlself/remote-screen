import { act, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { FakeIpc } from '../../../ipc/fake';
import type { FakeState } from '../../../ipc/fixtures';
import { flush, renderPage } from '../../../ipc/testing';

import NetworkSection from './index';

function createIpc(init?: Partial<FakeState>): FakeIpc {
  return new FakeIpc(init);
}

function callsTo(ipc: FakeIpc, method: string): Array<{ method: string; params: unknown }> {
  return ipc.calls.filter((call) => call.method === method);
}

function lastSet(ipc: FakeIpc): unknown {
  const calls = callsTo(ipc, 'settings.set');
  return calls.at(-1)?.params;
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('NetworkSection', () => {
  it('renders the default state without loading nic details', async () => {
    const ipc = createIpc();
    renderPage(<NetworkSection />, { ipc });
    await flush();

    expect(screen.getByRole('heading', { level: 2, name: '网络与访问' })).toBeInTheDocument();
    expect(screen.getByText('通过 tailnet')).toBeInTheDocument();
    expect(screen.getByText('100.64.0.7')).toBeInTheDocument();
    expect(screen.getByText('已连接')).toBeInTheDocument();
    expect(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' })).toHaveAttribute(
      'aria-checked',
      'false',
    );
    expect(screen.queryByText('监听的网卡')).not.toBeInTheDocument();
    expect(callsTo(ipc, 'network.listNics')).toHaveLength(0);
    expect(screen.getByRole('switch', { name: '内网来的连接每次都要本机确认' })).toBeDisabled();
  });

  it('saves enabled, then loads nics and firewall', async () => {
    const ipc = createIpc();
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    expect(lastSet(ipc)).toEqual({ intranet: { enabled: true } });
    expect(callsTo(ipc, 'network.listNics')).toHaveLength(1);
    expect(callsTo(ipc, 'network.firewallStatus')).toHaveLength(1);

    const en0 = screen.getByRole('checkbox', { name: 'en0' });
    expect(en0).toHaveAttribute('aria-checked', 'true');
    expect(screen.getAllByText('192.168.31.20').length).toBeGreaterThan(0);

    const en1 = screen.getByRole('checkbox', { name: 'en1' });
    expect(en1).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(en1).toBeDisabled();
    expect(screen.queryByRole('checkbox', { name: 'utun4' })).not.toBeInTheDocument();
  });

  it('lists a configured but missing interface so it can be unchecked', async () => {
    const ipc = createIpc();
    ipc.state.settings.intranet.enabled = true;
    ipc.state.settings.intranet.interfaces = ['en0', 'en9'];
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();

    expect(screen.getAllByText('找不到这块网卡').length).toBeGreaterThan(0);
    await user.click(screen.getByRole('checkbox', { name: 'en9找不到这块网卡' }));
    await flush();
    expect(lastSet(ipc)).toEqual({ intranet: { interfaces: ['en0'] } });
  });

  it('unchecks en0 and shows the no-interface warning', async () => {
    const ipc = createIpc();
    ipc.state.settings.intranet.enabled = true;
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('checkbox', { name: 'en0' }));
    await flush();

    expect(lastSet(ipc)).toEqual({ intranet: { interfaces: [] } });
    expect(screen.getByText('没有选网卡，内网连接不会生效')).toBeInTheDocument();
  });

  it('validates and mutates allow_sources', async () => {
    const ipc = createIpc();
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    expect(screen.getByText('10.30.0.0/16')).toBeInTheDocument();
    expect(screen.getByText('192.168.31.0/24')).toBeInTheDocument();

    const input = screen.getByRole('textbox', { name: '新的来源网段' });
    await user.type(input, '10.30.12.0/24');
    await user.click(screen.getByRole('button', { name: '添加' }));
    await flush();
    expect(lastSet(ipc)).toEqual({
      intranet: {
        allow_sources: ['10.30.0.0/16', '192.168.31.0/24', '10.30.12.0/24'],
      },
    });

    await user.type(input, '10.30.12.0');
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByText('格式不对，要写成 10.30.0.0/16 这样')).toBeInTheDocument();

    await user.clear(input);
    await user.type(input, '10.30.0.0/16');
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByText('已经在列表里了')).toBeInTheDocument();
  });

  it('shows a warning when no source networks remain', async () => {
    const ipc = createIpc();
    ipc.state.settings.intranet.enabled = true;
    ipc.state.settings.intranet.allow_sources = [];
    renderPage(<NetworkSection />, { ipc });
    await flush();

    expect(screen.getByText('没有允许的来源，内网来的连接都会被拒绝')).toBeInTheDocument();
  });

  it('removes an allow source', async () => {
    const ipc = createIpc();
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    await user.click(screen.getByRole('button', { name: '移除 10.30.0.0/16' }));
    await flush();
    expect(lastSet(ipc)).toEqual({ intranet: { allow_sources: ['192.168.31.0/24'] } });
  });

  it('copies the phone address and resets after two seconds', async () => {
    vi.useFakeTimers();
    const ipc = createIpc();
    const { user, platform } = renderPage(<NetworkSection />, { ipc });

    await flush();
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    const addressBlock = screen.getByText('在手机上添加这个地址').parentElement as HTMLElement;
    expect(within(addressBlock).getByText('192.168.31.20')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '复制' }));
    await flush();
    expect(platform.clipboard).toBe('192.168.31.20');
    expect(screen.getByRole('button', { name: '已复制' })).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(screen.getByRole('button', { name: '复制' })).toBeInTheDocument();
  });

  it.each([
    ['off', { enabled: false, active: false, detail: {} }, 'macOS 防火墙没有打开，不需要规则'],
    ['blocked', { enabled: true, active: false, detail: {} }, '防火墙规则被策略拦住'],
    ['active', { enabled: true, active: true, detail: {} }, '已添加防火墙规则'],
  ])('renders firewall state %s', async (_name, firewall, expected) => {
    const ipc = createIpc();
    ipc.state.settings.intranet.enabled = true;
    ipc.state.firewall = firewall;
    renderPage(<NetworkSection />, { ipc });
    await flush();

    expect(screen.getByText(expected)).toBeInTheDocument();
  });

  it('alerts when the IT note cannot be fetched', async () => {
    const ipc = createIpc();
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => undefined);
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    ipc.failNext('network.itNote', { code: -32000, message: 'unavailable' });
    await user.click(screen.getByRole('button', { name: '复制给 IT 的说明' }));
    await flush();

    expect(alert).toHaveBeenCalledWith('没拿到说明：unavailable');
  });

  it('copies the IT note', async () => {
    const ipc = createIpc();
    const { user, platform } = renderPage(<NetworkSection />, { ipc });
    await flush();
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    await user.click(screen.getByRole('button', { name: '复制给 IT 的说明' }));
    await flush();

    expect(callsTo(ipc, 'network.itNote')).toHaveLength(1);
    expect(platform.clipboard).toBe(ipc.state.itNote);
  });

  it('mutates confirm_each and allow_pairing', async () => {
    const ipc = createIpc();
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    await user.click(screen.getByRole('switch', { name: '内网来的连接每次都要本机确认' }));
    await flush();
    expect(lastSet(ipc)).toEqual({ intranet: { confirm_each: true } });

    await user.click(screen.getByRole('switch', { name: '允许新设备经内网配对' }));
    await flush();
    expect(lastSet(ipc)).toEqual({ intranet: { allow_pairing: true } });
  });

  it('rolls back a failed save', async () => {
    const ipc = createIpc();
    const { user } = renderPage(<NetworkSection />, { ipc });
    await flush();

    ipc.failNext('settings.set', { code: -32000, message: 'blocked' });
    await user.click(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' }));
    await flush();

    expect(screen.getByRole('switch', { name: '通过内网 / 局域网 IP' })).toHaveAttribute(
      'aria-checked',
      'false',
    );
    expect(screen.getByText('设置没有保存：blocked')).toBeInTheDocument();
  });

  it('warns when no route is available', async () => {
    const ipc = createIpc();
    ipc.state.status.tailnet_addresses = [];
    renderPage(<NetworkSection />, { ipc });
    await flush();

    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(screen.getByText('两条路都不通，手机现在连不上这台 Mac')).toBeInTheDocument();
  });

  it('tolerates malformed nic entries', async () => {
    const ipc = createIpc();
    ipc.state.settings.intranet.enabled = true;
    ipc.state.nics = [...ipc.state.nics, null, { name: 'en2', networks: [], tailnet: false }];
    renderPage(<NetworkSection />, { ipc });
    await flush();

    const en2 = screen.getByRole('checkbox', { name: 'en2' });
    expect(en2).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getAllByText('未连接').length).toBeGreaterThanOrEqual(2);
  });
});
