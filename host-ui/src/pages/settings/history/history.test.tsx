import { act, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { FakeIpc } from '../../../ipc/fake';
import { flush, renderPage } from '../../../ipc/testing';

import HistorySection from '.';

function dataRows(): HTMLElement[] {
  return within(screen.getByRole('table')).getAllByRole('row').slice(1);
}

function auditCalls(ipc: FakeIpc): Array<{ method: string; params: unknown }> {
  return ipc.calls.filter((call) => call.method === 'audit.list');
}

describe('HistorySection', () => {
  it('renders the fixture history with device names, events, paths and durations', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    renderPage(<HistorySection />, { ipc });
    await flush();

    expect(screen.getByRole('heading', { name: '连接记录' })).toBeInTheDocument();
    expect(screen.getByText('只保存在这台 Mac 上')).toBeInTheDocument();

    const rows = dataRows();
    expect(rows).toHaveLength(3);
    const first = rows[0] as HTMLElement;
    expect(within(first).getByText('小明的 iPhone')).toBeInTheDocument();
    expect(within(first).getByText('结束连接')).toBeInTheDocument();
    expect(within(first).getByText('直连')).toBeInTheDocument();
    expect(within(first).getByText('42:18')).toBeInTheDocument();
    expect(within(first).getByText('alice@')).toBeInTheDocument();

    const third = rows[2] as HTMLElement;
    expect(within(third).getByText('小明的 iPad')).toBeInTheDocument();
    expect(within(third).getByText('开始连接')).toBeInTheDocument();
    expect(within(third).getByText('局域网')).toBeInTheDocument();

    expect(auditCalls(ipc)[0]).toEqual({ method: 'audit.list', params: { limit: 50 } });
    expect(screen.queryByRole('button', { name: '加载更多' })).not.toBeInTheDocument();
  });

  it('loads the next page with the last visible timestamp and appends it', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const bulk = Array.from({ length: 60 }, (_, index) => ({
      ts: 2_000_000_000 - index * 1_000_000,
      user: 'alice@',
      device_id: 'k3jq7x2m9p4tbw8a',
      event: 'session.start',
      path: 'tailnet_direct',
      duration_s: null,
    }));
    ipc.state.audit = bulk;

    const firstView = renderPage(<HistorySection />, { ipc });
    await flush();
    firstView.unmount();

    renderPage(<HistorySection />, { ipc });
    await flush();

    expect(dataRows()).toHaveLength(50);
    const loadMore = screen.getByRole('button', { name: '加载更多' });

    await firstView.user.click(loadMore);
    await flush();

    expect(dataRows()).toHaveLength(60);
    expect(screen.queryByRole('button', { name: '加载更多' })).not.toBeInTheDocument();
    expect(auditCalls(ipc).at(-1)).toEqual({
      method: 'audit.list',
      params: { limit: 50, before_us: bulk[49]?.ts },
    });
  });

  it('resets to the first page when a sessions changed event refreshes the list', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const bulk = Array.from({ length: 60 }, (_, index) => ({
      ts: 2_000_000_000 - index * 1_000_000,
      user: null,
      device_id: null,
      event: 'auth.decision',
      path: 'intranet',
      duration_s: null,
    }));
    ipc.state.audit = bulk;
    const { user } = renderPage(<HistorySection />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '加载更多' }));
    await flush();
    expect(dataRows()).toHaveLength(60);

    act(() => {
      ipc.emit('event.sessionsChanged', {});
    });
    await flush();

    expect(dataRows()).toHaveLength(50);
    expect(screen.getByRole('button', { name: '加载更多' })).toBeInTheDocument();
  });

  it('still renders history when the device list fails', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.failNext('devices.list', {
      code: -32000,
      message: 'devices unavailable',
      data: { code: 'unavailable' },
    });
    renderPage(<HistorySection />, { ipc });
    await flush();

    expect(dataRows()).toHaveLength(3);
    expect(screen.getAllByText('k3jq7x2m9p4tbw8a').length).toBeGreaterThan(0);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('drops malformed audit entries without breaking the table', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.state.audit = [...ipc.state.audit, { ts: 'x' }, null];
    renderPage(<HistorySection />, { ipc });
    await flush();

    expect(dataRows()).toHaveLength(3);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows unknown devices verbatim and labels security events', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.state.audit = [
      {
        ts: 2_000_000_000_000_000,
        user: null,
        device_id: 'unknown-device',
        event: 'security.fp_mismatch',
        path: null,
        duration_s: null,
      },
    ];
    renderPage(<HistorySection />, { ipc });
    await flush();

    const row = dataRows()[0] as HTMLElement;
    expect(within(row).getByText('unknown-device')).toBeInTheDocument();
    expect(within(row).getByText('安全事件')).toBeInTheDocument();
    expect(within(row).queryByText('—')).not.toBeInTheDocument();
  });

  it('shows an empty state and recovers from an audit error with retry', async () => {
    const emptyIpc = FakeIpc.fromScenario('controlled');
    emptyIpc.state.audit = [];
    const emptyView = renderPage(<HistorySection />, { ipc: emptyIpc });
    await flush();

    expect(screen.getByText('还没有连接记录')).toBeInTheDocument();
    emptyView.unmount();

    const ipc = FakeIpc.fromScenario('controlled');
    ipc.failNext('audit.list', {
      code: -32000,
      message: 'audit unavailable',
      data: { code: 'unavailable' },
    });
    const { user } = renderPage(<HistorySection />, { ipc });
    await flush();

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('读不到连接记录：audit unavailable');
    await user.click(screen.getByRole('button', { name: '重试' }));
    await flush();

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(dataRows()).toHaveLength(3);
  });
});
