import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { FakeIpc } from '../../../ipc/fake';
import { flush, renderPage } from '../../../ipc/testing';

import AboutSection from '.';

describe('AboutSection', () => {
  it('shows the version and tailnet address and opens the update window', async () => {
    const { platform, user } = renderPage(<AboutSection />);
    await flush();

    expect(screen.getByText('0.3.1')).toBeInTheDocument();
    expect(screen.getByText('100.64.0.7')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '软件更新…' }));
    await flush();

    expect(platform.calls).toContainEqual({ op: 'showWindow', args: ['update', 'update'] });
  });

  it('does not show a Hub URL that an older Host still returns', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    Object.assign(ipc.state.settings, {
      hub: { url: 'http://rs-hub:7300', thumbnail: true, crash_reports: false },
    });
    renderPage(<AboutSection />, { ipc });
    await flush();

    expect(screen.getByText('100.64.0.7')).toBeInTheDocument();
    expect(screen.queryByText(/hub/i)).not.toBeInTheDocument();
  });

  it('exports masked logs by default and reveals the exported file', async () => {
    const { ipc, platform, user } = renderPage(<AboutSection />);
    await flush();

    expect(screen.getByRole('checkbox', { name: '隐藏设备名和地址' })).toHaveAttribute(
      'aria-checked',
      'true',
    );

    await user.click(screen.getByRole('button', { name: '导出日志…' }));
    await flush();

    expect(ipc.calls).toContainEqual({ method: 'logs.export', params: { mask: true } });
    expect(screen.getByText('已导出到 /tmp/remote-host-logs.zip')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '在 Finder 中显示' }));
    await flush();

    expect(platform.calls).toContainEqual({
      op: 'revealPath',
      args: ['/tmp/remote-host-logs.zip'],
    });
  });

  it('exports unmasked logs when the checkbox is cleared', async () => {
    const { ipc, user } = renderPage(<AboutSection />);
    await flush();

    await user.click(screen.getByRole('checkbox', { name: '隐藏设备名和地址' }));
    await user.click(screen.getByRole('button', { name: '导出日志…' }));
    await flush();

    expect(ipc.calls).toContainEqual({ method: 'logs.export', params: { mask: false } });
  });

  it('reports a failed export as an alert', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.failNext('logs.export', { code: -32603, message: 'permission denied' });
    const { user } = renderPage(<AboutSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '导出日志…' }));
    await flush();

    expect(screen.getByRole('alert')).toHaveTextContent('没有导出：permission denied');
    expect(screen.getByRole('button', { name: '导出日志…' })).toBeEnabled();
  });
});
