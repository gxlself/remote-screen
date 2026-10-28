import { screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { parseHash } from '../../app/router';
import { FakeIpc } from '../../ipc/fake';
import { flush, renderPage } from '../../ipc/testing';

import SettingsPage from '.';

function renderSettings(hash: string) {
  return renderPage(<SettingsPage route={parseHash(hash)} />, { hash });
}

describe('SettingsPage', () => {
  it('uses general for a missing or unknown section', async () => {
    const { unmount } = renderSettings('#/settings');
    await flush();

    expect(screen.getByRole('button', { name: '通用' })).toHaveAttribute('aria-current', 'page');
    expect(document.querySelector('[data-section="general"]')).not.toBeNull();

    unmount();
    renderSettings('#/settings/nope');
    await flush();

    expect(screen.getByRole('button', { name: '通用' })).toHaveAttribute('aria-current', 'page');
    expect(document.querySelector('[data-section="general"]')).not.toBeNull();
  });

  it('renders the section selected by the route', async () => {
    renderSettings('#/settings/devices');
    await flush();

    expect(screen.getByRole('button', { name: '已配对的设备' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    expect(document.querySelector('[data-section="devices"]')).not.toBeNull();
  });

  it('renders the seven sections in order, the paired device count and navigates on click', async () => {
    const { user } = renderSettings('#/settings');
    await flush();

    const nav = screen.getByRole('navigation', { name: '设置分类' });
    const buttons = within(nav).getAllByRole('button');

    expect(buttons.map((button) => button.textContent)).toEqual([
      '通用',
      '网络与访问',
      '已配对的设备3',
      '配对方式',
      '权限',
      '连接记录',
      '关于',
    ]);
    expect(within(buttons[2] as HTMLElement).getByText('3')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '权限' }));
    expect(location.hash).toBe('#/settings/permissions');
  });

  it('shows the version and first tailnet address in the sidebar', async () => {
    renderSettings('#/settings');
    await flush();

    expect(screen.getByText('0.3.1')).toBeInTheDocument();
    expect(screen.getByText('100.64.0.7')).toBeInTheDocument();
    expect(screen.getByText('这台 Mac')).toBeInTheDocument();
  });

  it('shows a banner above the section when the agent is offline', async () => {
    renderPage(<SettingsPage route={parseHash('#/settings')} />, {
      hash: '#/settings',
      ipc: FakeIpc.fromScenario('offline'),
    });
    await flush();

    expect(screen.getByText('Remote Host 服务没有运行')).toBeInTheDocument();
    expect(screen.getByText('稍等几秒会自动重连')).toBeInTheDocument();
    expect(document.querySelector('[data-section="general"]')).not.toBeNull();
  });
});
