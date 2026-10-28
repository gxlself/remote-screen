import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { FakeIpc } from '../../../ipc/fake';
import { flush, renderPage } from '../../../ipc/testing';

import GeneralSection from '.';

function settingsCalls(ipc: FakeIpc) {
  return ipc.calls.filter((call) => call.method === 'settings.set');
}

describe('GeneralSection', () => {
  it('optimistically saves an access switch', async () => {
    const { ipc, user } = renderPage(<GeneralSection />);
    await flush();

    const allow = screen.getByRole('switch', { name: '允许被远程控制' });
    expect(allow).toHaveAttribute('aria-checked', 'true');

    await user.click(allow);
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { access: { allow_remote_control: false } },
    });
    expect(allow).toHaveAttribute('aria-checked', 'false');
  });

  it('rolls an access switch back and shows an alert when saving fails', async () => {
    const { ipc, user } = renderPage(<GeneralSection />);
    await flush();

    ipc.failNext('settings.set', { code: -32603, message: 'disk full' });
    await user.click(screen.getByRole('switch', { name: '允许被远程控制' }));
    await flush();

    expect(screen.getByRole('switch', { name: '允许被远程控制' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('alert')).toHaveTextContent('设置没有保存：disk full');
  });

  it('saves the selected idle timeout', async () => {
    const { ipc, user } = renderPage(<GeneralSection />);
    await flush();

    await user.selectOptions(screen.getByRole('combobox', { name: '没人操作多久后断开' }), '60');
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { access: { idle_timeout_min: 60 } },
    });
  });

  it('adds an extra idle timeout option for an unexpected saved value', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.state.settings.access.idle_timeout_min = 45;
    renderPage(<GeneralSection />, { ipc });
    await flush();

    expect(screen.getByRole('option', { name: '45 分钟' })).toBeInTheDocument();
  });

  it('adds, validates and removes allowed tailnet accounts', async () => {
    const { ipc, user } = renderPage(<GeneralSection />);
    await flush();

    expect(screen.getByText('alice@')).toBeInTheDocument();
    const input = screen.getByRole('textbox', { name: 'tailnet 账号' });

    await user.type(input, 'lisi@');
    await user.click(screen.getByRole('button', { name: '添加' }));
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { access: { allowed_users: ['alice@', 'lisi@'] } },
    });
    expect(screen.getByText('lisi@')).toBeInTheDocument();

    const callsBeforeDuplicate = settingsCalls(ipc).length;
    await user.type(input, 'alice@');
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByRole('alert')).toHaveTextContent('已经在列表里了');
    expect(settingsCalls(ipc)).toHaveLength(callsBeforeDuplicate);

    await user.clear(input);
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByRole('alert')).toHaveTextContent('请输入账号');

    await user.type(input, 'a b');
    await user.click(screen.getByRole('button', { name: '添加' }));
    expect(screen.getByRole('alert')).toHaveTextContent('账号里不能有空格');
  });

  it('removes the last account and explains the empty list', async () => {
    const { ipc, user } = renderPage(<GeneralSection />);
    await flush();

    await user.click(screen.getByRole('button', { name: '移除 alice@' }));
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { access: { allowed_users: [] } },
    });
    expect(screen.getByText('只允许这台 Mac 上登录 Tailscale 的账号')).toBeInTheDocument();
  });

  it('saves both clipboard switches', async () => {
    const { ipc, user } = renderPage(<GeneralSection />);
    await flush();

    await user.click(screen.getByRole('switch', { name: '允许把这台 Mac 的剪贴板发到手机' }));
    await flush();
    await user.click(screen.getByRole('switch', { name: '允许接收手机发来的剪贴板' }));
    await flush();

    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { clipboard: { to_phone: false } },
    });
    expect(settingsCalls(ipc)).toContainEqual({
      method: 'settings.set',
      params: { clipboard: { from_phone: false } },
    });
  });

  it('ignores a hub section that an older Host still returns', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    Object.assign(ipc.state.settings, {
      hub: { url: 'http://rs-hub:7300', thumbnail: true, crash_reports: false },
    });
    const { user } = renderPage(<GeneralSection />, { ipc });
    await flush();

    const groups = screen.getAllByRole('heading', { level: 3 });
    expect(groups.map((heading) => heading.textContent)).toEqual(['访问', '剪贴板']);
    expect(screen.queryByText(/hub/i)).not.toBeInTheDocument();

    await user.click(screen.getByRole('switch', { name: '允许被远程控制' }));
    await flush();

    expect(settingsCalls(ipc)).toEqual([
      { method: 'settings.set', params: { access: { allow_remote_control: false } } },
    ]);
    expect(screen.getByRole('switch', { name: '允许被远程控制' })).toHaveAttribute(
      'aria-checked',
      'false',
    );
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
