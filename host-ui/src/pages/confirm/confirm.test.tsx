import { act, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ApprovalRequest } from '../../ipc';
import { FakeIpc } from '../../ipc/fake';
import { APPROVAL_CONNECTION, APPROVAL_PAIRING } from '../../ipc/fixtures';
import { flush, renderPage } from '../../ipc/testing';
import { FakePlatform } from '../../platform';

import ConfirmPage, { approvalDeviceValue } from '.';

function renderConfirm(ipc = FakeIpc.fromScenario('idle'), platform = new FakePlatform()) {
  return renderPage(<ConfirmPage route={{ page: 'confirm', query: {} }} />, { ipc, platform });
}

function approvalRequests(ipc: FakeIpc) {
  return ipc.state.approvals;
}

function outcomeEvent(ipc: FakeIpc, requestId: string, kind: ApprovalRequest['kind']) {
  ipc.emit('event.approvalClosed', { request_id: requestId, kind, outcome: 'allowed' });
}

describe('ConfirmPage', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('starts idle without a dialog or window calls', async () => {
    const { platform } = renderConfirm();
    await flush();

    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(platform.calls.some((call) => call.op === 'showWindow')).toBe(false);
  });

  it('shows a pairing request with identity, fingerprint and safe initial focus', async () => {
    const { ipc, platform } = renderConfirm();

    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();
    await act(async () => {
      vi.advanceTimersByTime(0);
    });

    const dialog = screen.getByRole('alertdialog');
    expect(platform.calls).toContainEqual(expect.objectContaining({ op: 'showWindow' }));
    expect(platform.calls.find((call) => call.op === 'showWindow')?.args[0]).toBe('confirm');
    expect(
      within(dialog).getByRole('heading', { name: '「小明的 iPhone」想要连接这台 Mac' }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText('alice@')).toBeInTheDocument();
    expect(within(dialog).getByText('tailnet 已验证')).toBeInTheDocument();
    // D39 统一指纹显示
    expect(within(dialog).getByText('KH71 PYG0 0000 0000')).toBeInTheDocument();
    expect(within(dialog).getByRole('radio', { name: /允许控制/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('button', { name: '拒绝' })).toHaveFocus();
  });

  it('allows a pairing request with the default control permission and hides the window', async () => {
    const { ipc, platform, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();

    expect(approvalRequests(ipc)).toEqual([
      { request_id: 'r-1', allow: true, permission: 'control' },
    ]);
    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['confirm'] });
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });

  it('maps unattended and view-only pairing choices to permissions', async () => {
    const { ipc, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    const unattended = screen.getByRole('checkbox', { name: /允许无人值守/ });
    await user.click(unattended);
    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();
    expect(approvalRequests(ipc)[0]).toMatchObject({ permission: 'unattended' });

    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_PAIRING, request_id: 'r-3' });
    });
    await flush();

    await user.click(screen.getByRole('radio', { name: /仅观看/ }));
    await flush();
    expect(screen.getByRole('checkbox', { name: /允许无人值守/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(screen.getByRole('checkbox', { name: /允许无人值守/ })).toHaveAttribute(
      'aria-checked',
      'false',
    );
    await user.click(screen.getByRole('button', { name: '允许观看' }));
    await flush();
    expect(approvalRequests(ipc)[1]).toEqual({
      request_id: 'r-3',
      allow: true,
      permission: 'view_only',
    });
  });

  it('denies by button and Escape, but never allows on Enter', async () => {
    const { ipc, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    await user.click(screen.getByRole('button', { name: '拒绝' }));
    await flush();
    expect(approvalRequests(ipc)).toEqual([{ request_id: 'r-1', allow: false }]);
    expect(approvalRequests(ipc)[0]).not.toHaveProperty('permission');

    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_PAIRING, request_id: 'r-2' });
    });
    await flush();
    await user.keyboard('{Escape}');
    await flush();
    expect(approvalRequests(ipc)[1]).toEqual({ request_id: 'r-2', allow: false });

    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_PAIRING, request_id: 'r-3' });
    });
    await flush();
    await user.keyboard('{Enter}');
    await flush();
    expect(approvalRequests(ipc)).toHaveLength(2);
  });

  it('counts down from the payload timeout and removes an expired request without responding', async () => {
    const { ipc, platform } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();
    expect(screen.getByText('30 秒后自动拒绝')).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(2_000);
    });
    await flush();
    expect(screen.getByText('28 秒后自动拒绝')).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(28_000);
    });
    await flush();

    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(approvalRequests(ipc)).toEqual([]);
    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['confirm'] });
  });

  it('renders a non-pairing attended connection and allows it without permission', async () => {
    const { ipc, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_CONNECTION);
    });
    await flush();

    const dialog = screen.getByRole('alertdialog');
    expect(within(dialog).queryByRole('radiogroup')).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent('这台 Mac 设成了每次连接都要确认。');
    expect(within(dialog).queryByText('设备')).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent('直连');
    expect(
      approvalDeviceValue({
        ...APPROVAL_CONNECTION,
        node_name: 'xl-ipad',
        peer_addr: '100.64.0.21',
      } as ApprovalRequest),
    ).toBe('xl-ipad · 100.64.0.21');

    await user.click(within(dialog).getByRole('button', { name: '允许' }));
    await flush();
    expect(approvalRequests(ipc)).toEqual([{ request_id: 'r-2', allow: true }]);
    expect(approvalRequests(ipc)[0]).not.toHaveProperty('permission');
  });

  it.each([
    ['local_activity', '你刚刚在用这台 Mac，所以先问你一下。'],
    ['intranet_confirmation', '这次是从内网连进来的，需要你确认。'],
    ['node_mismatch', '这台设备的 tailnet 节点和配对时记录的不一样，请确认是你认识的设备。'],
    ['unexpected_reason', '确认后才会开始。'],
  ])('maps the %s approval reason to its copy', async (reason, expected) => {
    const { ipc } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_CONNECTION, reasons: [reason] });
    });
    await flush();

    expect(screen.getByRole('alertdialog')).toHaveTextContent(expected);
  });

  it('queues requests in arrival order, deduplicates ids and advances after an answer', async () => {
    const { ipc, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
      ipc.emit('event.approvalRequested', APPROVAL_CONNECTION);
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    expect(screen.getByText('还有 1 个请求在等')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /小明的 iPhone/ })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();
    expect(screen.getByRole('heading', { name: /小明的 iPad/ })).toBeInTheDocument();
    expect(screen.queryByText('还有 1 个请求在等')).not.toBeInTheDocument();
  });

  it('resets pairing choices for the next queued request', async () => {
    const { ipc, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
      ipc.emit('event.approvalRequested', { ...APPROVAL_PAIRING, request_id: 'r-3' });
    });
    await flush();

    await user.click(screen.getByRole('radio', { name: /仅观看/ }));
    await user.click(screen.getByRole('button', { name: '允许观看' }));
    await flush();

    expect(screen.getByRole('radio', { name: /允许控制/ })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('button', { name: '允许控制' })).toBeInTheDocument();
  });

  it('renders takeover copy with the current controller from sessions.list', async () => {
    const takeover = {
      ...APPROVAL_CONNECTION,
      request_id: 'r-control',
      kind: 'takeover',
      device_name: '小明的 iPad',
    } satisfies ApprovalRequest;
    const { ipc } = renderConfirm(FakeIpc.fromScenario('controlled'));
    act(() => {
      ipc.emit('event.approvalRequested', takeover);
    });
    await flush();

    expect(
      screen.getByRole('heading', { name: '「小明的 iPad」想要接管控制' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('alertdialog')).toHaveTextContent('现在由 小明的 iPhone 控制');
  });

  it('advances on not_found and keeps the request visible on other errors', async () => {
    const { ipc, user } = renderConfirm();
    ipc.failNext('approval.respond', {
      code: -32000,
      message: 'not found',
      data: { code: 'not_found' },
    });
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();

    ipc.failNext('approval.respond', {
      code: -32000,
      message: 'disk full',
      data: { code: 'io' },
    });
    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_PAIRING, request_id: 'r-4' });
    });
    await flush();

    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();
    expect(screen.getByRole('alert')).toHaveTextContent('没有发出去：disk full');
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();
    expect(approvalRequests(ipc).at(-1)).toMatchObject({ request_id: 'r-4', allow: true });
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });

  it('clears the queue and hides the window when IPC disconnects', async () => {
    const { ipc, platform } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    act(() => {
      ipc.setConnected(false);
    });
    await flush();

    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['confirm'] });
  });

  it('shows a pending request fetched after connecting', async () => {
    const ipc = FakeIpc.fromScenario('idle');
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });

    renderConfirm(ipc);
    await flush();

    expect(ipc.calls).toContainEqual({ method: 'approval.pending', params: null });
    expect(screen.getByRole('heading', { name: /小明的 iPhone/ })).toBeInTheDocument();
    expect(screen.getByText('30 秒后自动拒绝')).toBeInTheDocument();
  });

  it('deduplicates a pending request that also arrives in real time', async () => {
    const ipc = FakeIpc.fromScenario('idle');
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });

    renderConfirm(ipc);
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    expect(screen.getAllByRole('alertdialog')).toHaveLength(1);
    expect(screen.queryByText(/还有 .* 个请求在等/)).not.toBeInTheDocument();
  });

  it('fetches pending requests again after reconnecting', async () => {
    const ipc = FakeIpc.fromScenario('idle');
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    renderConfirm(ipc);
    await flush();
    expect(ipc.calls.filter((call) => call.method === 'approval.pending')).toHaveLength(1);
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();

    act(() => {
      ipc.setConnected(false);
    });
    await flush();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    act(() => {
      ipc.setConnected(true);
    });
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'approval.pending')).toHaveLength(2);
    expect(screen.getByRole('heading', { name: /小明的 iPhone/ })).toBeInTheDocument();
  });

  it('waits until an initially disconnected IPC connects before fetching', async () => {
    const ipc = FakeIpc.fromScenario('idle');
    ipc.setConnected(false);
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });

    renderConfirm(ipc);
    await flush();

    expect(ipc.calls.some((call) => call.method === 'approval.pending')).toBe(false);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();

    act(() => {
      ipc.setConnected(true);
    });
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'approval.pending')).toHaveLength(1);
    expect(screen.getByRole('heading', { name: /小明的 iPhone/ })).toBeInTheDocument();
  });

  it('keeps showing real-time requests when approval.pending fails', async () => {
    const debug = vi.spyOn(console, 'debug').mockImplementation(() => undefined);
    const ipc = FakeIpc.fromScenario('idle');
    ipc.failNext('approval.pending', {
      code: -32000,
      message: 'temporarily unavailable',
      data: { code: 'unavailable' },
    });

    renderConfirm(ipc);
    await flush();

    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    expect(screen.getByRole('heading', { name: /小明的 iPhone/ })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(debug).toHaveBeenCalledWith('approval.pending failed');
  });

  it('shows an em dash and no verified tag for a null whois login', async () => {
    const { ipc } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_CONNECTION, whois_login: null });
    });
    await flush();

    const dialog = screen.getByRole('alertdialog');
    const account = within(dialog).getByText('账号').closest('dl');
    expect(account).toHaveTextContent('—');
    expect(within(dialog).queryByText('tailnet 已验证')).not.toBeInTheDocument();
  });

  it('uses the payload timeout rather than a hard-coded 30 seconds', async () => {
    const { ipc, platform } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', { ...APPROVAL_CONNECTION, timeout_s: 5 });
    });
    await flush();
    expect(screen.getByText('5 秒后自动拒绝')).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(5_000);
    });
    await flush();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['confirm'] });
  });

  it('keeps an independent deadline for each queued request', async () => {
    const { ipc, user } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
      ipc.emit('event.approvalRequested', { ...APPROVAL_CONNECTION, timeout_s: 5 });
    });
    await flush();

    await act(async () => {
      vi.advanceTimersByTime(4_000);
    });
    await user.click(screen.getByRole('button', { name: '允许控制' }));
    await flush();

    expect(screen.getByText('1 秒后自动拒绝')).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(1_000);
    });
    await flush();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });

  it('closes the displayed request when approvalClosed arrives', async () => {
    const { ipc } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
      ipc.emit('event.approvalRequested', APPROVAL_CONNECTION);
    });
    await flush();

    act(() => {
      outcomeEvent(ipc, 'r-1', 'pairing');
    });
    await flush();

    expect(screen.getByRole('heading', { name: /小明的 iPad/ })).toBeInTheDocument();
  });

  it('uses the fallback device name and hides optional rows when absent', async () => {
    const { ipc } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', {
        ...APPROVAL_CONNECTION,
        device_name: '',
        path: null,
        device_fp: null,
      });
    });
    await flush();

    const dialog = screen.getByRole('alertdialog');
    expect(
      within(dialog).getByRole('heading', { name: '「一台设备」想要连接这台 Mac' }),
    ).toBeInTheDocument();
    expect(within(dialog).queryByText('网络')).not.toBeInTheDocument();
    expect(within(dialog).queryByText('设备')).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent('—');
  });

  it('shows the Windows unattended and lock-screen guidance', async () => {
    const { ipc } = renderConfirm(
      FakeIpc.fromScenario('idle'),
      new FakePlatform({ os: 'windows' }),
    );
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    expect(
      screen.getByText('以后这台手机连进来直接开始，不再弹窗。可在设置里改回来。'),
    ).toBeInTheDocument();
    expect(screen.getByText('电脑锁屏时，这个提示会出现在登录界面上')).toBeInTheDocument();
  });

  it('does not show Windows-only guidance on macOS', async () => {
    const { ipc } = renderConfirm();
    act(() => {
      ipc.emit('event.approvalRequested', APPROVAL_PAIRING);
    });
    await flush();

    expect(
      screen.queryByText('以后这台手机连进来直接开始，不再弹窗。可在设置里改回来。'),
    ).not.toBeInTheDocument();
    expect(screen.queryByText('电脑锁屏时，这个提示会出现在登录界面上')).not.toBeInTheDocument();
  });
});
