import { screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { RouteView } from '../../../app/RouteView';
import { FakeIpc } from '../../../ipc/fake';
import { flush, renderPage } from '../../../ipc/testing';

import PairingSection from '.';

const USED_DEVICE_ID = 'k3jq7x2m9p4tbw8a';

function settingsCalls(ipc: FakeIpc) {
  return ipc.calls.filter((call) => call.method === 'settings.set');
}

function pairingCalls(ipc: FakeIpc) {
  return ipc.calls.filter((call) => call.method.startsWith('pairing.'));
}

describe('PairingSection', () => {
  it('is reachable from settings navigation and renders its three blocks', async () => {
    const { user } = renderPage(<RouteView />, { hash: '#/settings' });
    await flush();

    const nav = screen.getByRole('navigation', { name: '设置分类' });
    expect(
      within(nav)
        .getAllByRole('button')
        .map((button) => button.textContent),
    ).toEqual(['通用', '网络与访问', '已配对的设备3', '配对方式', '权限', '连接记录', '关于']);

    const pairingButton = screen.getByRole('button', { name: '配对方式' });
    await user.click(pairingButton);

    expect(location.hash).toBe('#/settings/pairing');

    await waitFor(() => {
      expect(document.querySelector('[data-section="pairing"]')).not.toBeNull();
    });

    expect(screen.getByRole('button', { name: '配对方式' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(screen.getByRole('heading', { name: '账户密码' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '配对文件' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '已导出的文件' })).toBeInTheDocument();
  });

  it('saves the password switch and permission, rolling back when saving fails', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    const passwordSwitch = screen.getByRole('switch', { name: '允许用这台 Mac 的登录密码配对' });
    expect(passwordSwitch).toHaveAttribute('aria-checked', 'false');
    const passwordBlock = document.querySelector('[data-pairing-block="password"]');
    expect(passwordBlock).not.toBeNull();
    expect(
      within(passwordBlock as HTMLElement).queryByRole('radio', { name: /允许控制/ }),
    ).toBeNull();

    await user.click(passwordSwitch);
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { security: { password_pairing: true } },
    });
    expect(passwordSwitch).toHaveAttribute('aria-checked', 'true');

    await user.click(within(passwordBlock as HTMLElement).getByRole('radio', { name: /仅观看/ }));
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { security: { password_pairing_permission: 'view_only' } },
    });
    expect(
      within(passwordBlock as HTMLElement).getByRole('radio', { name: /仅观看/ }),
    ).toHaveAttribute('aria-checked', 'true');

    ipc.failNext('settings.set', { code: -32603, message: 'disk full' });
    await user.click(within(passwordBlock as HTMLElement).getByRole('radio', { name: /无人值守/ }));
    await flush();

    expect(
      within(passwordBlock as HTMLElement).getByRole('radio', { name: /仅观看/ }),
    ).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('设置没有保存：disk full');
  });

  it('defaults missing security settings to control and disabled password pairing', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    delete ipc.state.settings.security;
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    expect(screen.getByRole('switch', { name: '允许用这台 Mac 的登录密码配对' })).toHaveAttribute(
      'aria-checked',
      'false',
    );

    await user.click(screen.getByRole('switch', { name: '允许用这台 Mac 的登录密码配对' }));
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { security: { password_pairing: true } },
    });
    const savedSecurity: unknown = ipc.state.settings.security;
    expect(savedSecurity).toMatchObject({ password_pairing_permission: 'control' });
  });

  it('exports a file with a label and view-only permission, then reveals it', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform, user } = renderPage(<PairingSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('radio', { name: /仅观看/ }));
    await user.type(screen.getByRole('textbox', { name: '备注' }), '现场支持');
    await user.click(screen.getByRole('button', { name: '导出' }));
    await flush();

    const exportCall = pairingCalls(ipc).find((call) => call.method === 'pairing.exportFile');
    expect(exportCall?.params).toEqual({ permission: 'view_only', label: '现场支持' });
    expect(screen.getByText('studio-mac-export-1.rspair')).toBeInTheDocument();
    expect(
      screen.getByText('/Users/example/Downloads/studio-mac-export-1.rspair'),
    ).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '在 Finder 中显示' }));
    await flush();

    expect(platform.calls).toContainEqual({
      op: 'revealPath',
      args: ['/Users/example/Downloads/studio-mac-export-1.rspair'],
    });
  });

  it('shows a clear error when too many pairing files exist', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.failNext('pairing.exportFile', {
      code: -32000,
      message: 'too many files',
      data: { code: 'too_many_files' },
    });
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '导出' }));
    await flush();

    expect(screen.getByRole('alert')).toHaveTextContent(
      '有效的配对文件太多了（最多 50 个），先撤销一些',
    );
  });

  it('shows the first six characters of the file id when there is no label', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '导出' }));
    await flush();

    expect(screen.getByText('文件 EEEEEE')).toBeInTheDocument();
  });

  it('shows io and generic export errors distinctly', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    ipc.failNext('pairing.exportFile', {
      code: -32000,
      message: 'permission denied',
      data: { code: 'io' },
    });
    await user.click(screen.getByRole('button', { name: '导出' }));
    await flush();
    expect(screen.getByRole('alert')).toHaveTextContent('写文件失败：permission denied');

    ipc.failNext('pairing.exportFile', { code: -32603, message: 'boom' });
    await user.click(screen.getByRole('button', { name: '导出' }));
    await flush();
    expect(screen.getByRole('alert')).toHaveTextContent('没有导出：boom');
  });

  it('lists preset files with their state and revokes an active one', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    const list = screen.getByRole('list');
    const rows = within(list).getAllByRole('listitem');
    expect(rows[0]).toHaveTextContent('IT 分发 10 月');
    expect(rows[0]).toHaveTextContent('有效');
    expect(rows[0]).toHaveTextContent('允许控制，每次确认');
    expect(rows[1]).toHaveTextContent('iPad 现场支持');
    expect(rows[1]).toHaveTextContent('已使用');
    expect(rows[1]).toHaveTextContent(USED_DEVICE_ID);

    await user.click(within(rows[0] as HTMLElement).getByRole('button', { name: '撤销' }));
    expect(screen.getByText('确定撤销这个配对文件？')).toBeInTheDocument();
    await user.click(within(rows[0] as HTMLElement).getByRole('button', { name: '撤销' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'pairing.revokeFile',
      params: { file_id: 'AAAAAAAAAAAAAAAAAAAAAA' },
    });
    const refreshed = within(screen.getByRole('list')).getAllByRole('listitem');
    expect(refreshed[0]).toHaveTextContent('已撤销');
    expect(within(refreshed[0] as HTMLElement).queryByRole('button', { name: '撤销' })).toBeNull();
  });

  it('shows the unsupported message when listFiles is not implemented', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.failNext('pairing.listFiles', {
      code: -32000,
      message: 'pairing.listFiles is not wired',
      data: { code: 'unsupported' },
    });
    renderPage(<PairingSection />, { ipc });
    await flush();

    expect(screen.getByText('这个版本的 Remote Host 还不支持配对文件')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '导出' })).toBeInTheDocument();
  });

  it('shows the unsupported message in the export block when export is not implemented', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.failNext('pairing.exportFile', {
      code: -32000,
      message: 'pairing.exportFile is not wired',
      data: { code: 'unsupported' },
    });
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '导出' }));
    await flush();

    expect(screen.getByText('这个版本的 Remote Host 还不支持配对文件')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '导出' })).not.toBeInTheDocument();
  });

  it('disables export and explains when the label exceeds 64 characters', async () => {
    const { user } = renderPage(<PairingSection />);
    await flush();

    const input = screen.getByRole('textbox', { name: '备注' });
    await user.type(input, 'a'.repeat(65));

    expect(screen.getByRole('button', { name: '导出' })).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent('备注最多 64 个字');
  });

  it('allows a 64-character label', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    const { user } = renderPage(<PairingSection />, { ipc });
    await flush();

    await user.type(screen.getByRole('textbox', { name: '备注' }), 'a'.repeat(64));
    expect(screen.getByRole('button', { name: '导出' })).toBeEnabled();
  });
});
