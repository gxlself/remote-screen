import { act, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FakeIpc } from '../../../ipc/fake';
import { flush, renderPage } from '../../../ipc/testing';

import DevicesSection from '.';

const NOW = new Date(2026, 9, 20, 15, 0);
const IPHONE_ID = 'k3jq7x2m9p4tbw8a';

function newIpc(): FakeIpc {
  return new FakeIpc(undefined, { now: () => Date.now() });
}

function deviceCalls(ipc: FakeIpc) {
  return ipc.calls.filter((call) => call.method.startsWith('devices.'));
}

function deviceButton(index: number): HTMLElement {
  const list = screen.getByRole('list');
  return within(list).getAllByRole('listitem')[index] as HTMLElement;
}

describe('DevicesSection', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('lists the paired devices with permission and path tags', async () => {
    renderPage(<DevicesSection />, { ipc: newIpc() });
    await flush();

    expect(screen.getByRole('heading', { name: '已配对的设备' })).toBeInTheDocument();
    expect(screen.getByText('3 台')).toBeInTheDocument();
    expect(deviceButton(0)).toHaveTextContent('小明的 iPhone');
    expect(deviceButton(1)).toHaveTextContent('小明的 iPad');
    expect(deviceButton(2)).toHaveTextContent('同事的 iPhone');
    expect(deviceButton(0)).toHaveTextContent('无人值守');
    expect(deviceButton(1)).toHaveTextContent('允许控制，每次确认');
    expect(screen.getByText('仅观看 · 只允许 tailnet')).toBeInTheDocument();
    expect(screen.getByText('还没连过')).toBeInTheDocument();
  });

  it('selects the first device and shows its details', async () => {
    renderPage(<DevicesSection />, { ipc: newIpc() });
    await flush();

    expect(deviceButton(0)).toHaveAttribute('aria-current', 'true');
    expect(deviceButton(1)).toHaveAttribute('aria-current', 'false');
    expect(screen.getByRole('radio', { name: /无人值守/ })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('checkbox', { name: /tailnet/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('checkbox', { name: /局域网/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('checkbox', { name: /内网 IP/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(
      within(screen.getByRole('region', { name: '设备详情' })).getByText(IPHONE_ID),
    ).toBeInTheDocument();
  });

  it('optimistically saves a permission change and updates the list tag', async () => {
    const ipc = newIpc();
    const { user } = renderPage(<DevicesSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('radio', { name: /仅观看/ }));
    await flush();

    expect(deviceCalls(ipc)).toContainEqual({
      method: 'devices.update',
      params: { device_id: IPHONE_ID, permission: 'view_only', paths: ['tailnet', 'lan'] },
    });
    expect(deviceButton(0)).toHaveTextContent('仅观看');
    expect(screen.getByRole('radio', { name: /仅观看/ })).toHaveAttribute('aria-checked', 'true');
  });

  it('sends all three fields, preserves path order, and refuses the last path', async () => {
    const ipc = newIpc();
    const { user } = renderPage(<DevicesSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('checkbox', { name: /局域网/ }));
    await flush();
    expect(ipc.calls).toContainEqual({
      method: 'devices.update',
      params: { device_id: IPHONE_ID, permission: 'unattended', paths: ['tailnet'] },
    });
    expect(deviceButton(0)).toHaveTextContent('无人值守 · 只允许 tailnet');

    const callsBefore = deviceCalls(ipc).length;
    await user.click(screen.getByRole('checkbox', { name: /tailnet/ }));
    await flush();

    expect(deviceCalls(ipc)).toHaveLength(callsBefore);
    expect(screen.getByRole('alert')).toHaveTextContent('至少留一条路，不然这台设备连不上');
    expect(screen.getByRole('checkbox', { name: /tailnet/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('rolls back and alerts when saving fails', async () => {
    const ipc = newIpc();
    const { user } = renderPage(<DevicesSection />, { ipc });
    await flush();

    ipc.failNext('devices.update', { code: -32000, message: 'locked', data: { code: 'io' } });
    await user.click(screen.getByRole('radio', { name: /仅观看/ }));
    await flush();

    expect(screen.getByRole('radio', { name: /无人值守/ })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('没有保存：locked');
  });

  it('switches the detail pane when another device is selected', async () => {
    const { user } = renderPage(<DevicesSection />, { ipc: newIpc() });
    await flush();

    await user.click(deviceButton(1));

    expect(deviceButton(1)).toHaveAttribute('aria-current', 'true');
    expect(screen.getByRole('radio', { name: /允许控制（每次确认）/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('supports ArrowDown and ArrowUp selection in the device list', async () => {
    const { user } = renderPage(<DevicesSection />, { ipc: newIpc() });
    await flush();

    deviceButton(0).focus();
    await user.keyboard('{ArrowDown}');
    expect(deviceButton(1)).toHaveAttribute('aria-current', 'true');
    expect(deviceButton(1)).toHaveFocus();

    await user.keyboard('{ArrowUp}');
    expect(deviceButton(0)).toHaveAttribute('aria-current', 'true');
    expect(deviceButton(0)).toHaveFocus();
  });

  it('confirms revoking a device and selects the next one', async () => {
    const ipc = newIpc();
    const { user } = renderPage(<DevicesSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '撤销配对' }));
    expect(screen.getByText('确定撤销「小明的 iPhone」？')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '取消' }));
    expect(deviceCalls(ipc).some((call) => call.method === 'devices.revoke')).toBe(false);

    await user.click(screen.getByRole('button', { name: '撤销配对' }));
    await user.click(screen.getByRole('button', { name: '撤销' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'devices.revoke',
      params: { device_id: IPHONE_ID },
    });
    expect(screen.getByText('2 台')).toBeInTheDocument();
    expect(deviceButton(0)).toHaveTextContent('小明的 iPad');
    expect(deviceButton(0)).toHaveAttribute('aria-current', 'true');
  });

  it('enables the intranet path only when the setting is on', async () => {
    const ipc = newIpc();
    ipc.state.settings.intranet.enabled = true;
    const { user } = renderPage(<DevicesSection />, { ipc });
    await flush();

    const intranet = screen.getByRole('checkbox', { name: /内网 IP/ });
    expect(intranet).toHaveAttribute('aria-disabled', 'false');
    expect(screen.queryByRole('button', { name: '去打开' })).not.toBeInTheDocument();

    await user.click(intranet);
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'devices.update',
      params: {
        device_id: IPHONE_ID,
        permission: 'unattended',
        paths: ['tailnet', 'lan', 'intranet'],
      },
    });
  });

  it('links to network settings when the intranet path is disabled', async () => {
    const { user } = renderPage(<DevicesSection />, { ipc: newIpc() });
    await flush();

    await user.click(screen.getByRole('button', { name: '去打开' }));
    expect(location.hash).toBe('#/settings/network');
  });

  it('opens the pairing window', async () => {
    const { platform, user } = renderPage(<DevicesSection />, { ipc: newIpc() });
    await flush();

    await user.click(screen.getByRole('button', { name: '配对新设备…' }));
    await flush();

    expect(platform.calls).toContainEqual({ op: 'showWindow', args: ['pair', 'pair'] });
  });

  it('renders an empty list without a detail pane in the setup scenario', async () => {
    renderPage(<DevicesSection />, { ipc: FakeIpc.fromScenario('setup') });
    await flush();

    expect(screen.getByText('还没有配对的设备')).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: '设备详情' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '撤销配对' })).not.toBeInTheDocument();
  });

  it('refetches the device list on pairingResult', async () => {
    const ipc = newIpc();
    renderPage(<DevicesSection />, { ipc });
    await flush();

    const before = ipc.calls.filter((call) => call.method === 'devices.list').length;
    act(() => {
      ipc.emit('event.pairingResult', {
        status: 1,
        host_id: 'h7v2m9q3k8x4tb1a',
        host_name: 'studio-mac',
        attempts_left: 0,
        device_fp: '',
      });
    });
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'devices.list')).toHaveLength(before + 1);
  });
});
