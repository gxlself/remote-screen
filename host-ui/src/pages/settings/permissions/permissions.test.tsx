import { act, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { SYSTEM_SETTINGS_URLS } from '../../../platform';
import { FakeIpc } from '../../../ipc/fake';
import { flush, renderPage } from '../../../ipc/testing';

import PermissionsSection from '.';

function permissionRow(key: string): HTMLElement {
  const row = document.querySelector<HTMLElement>(`[data-permission="${key}"]`);
  if (row === null) {
    throw new Error(`missing permission row: ${key}`);
  }
  return row;
}

describe('PermissionsSection', () => {
  it('shows both permissions as granted and no system settings buttons', async () => {
    renderPage(<PermissionsSection />);
    await flush();

    const screenRecording = permissionRow('screen_recording');
    const accessibility = permissionRow('accessibility');
    expect(within(screenRecording).getByText('屏幕录制')).toBeInTheDocument();
    expect(within(screenRecording).getByText('已授权')).toBeInTheDocument();
    expect(within(accessibility).getByText('辅助功能')).toBeInTheDocument();
    expect(within(accessibility).getByText('已授权')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '打开系统设置' })).not.toBeInTheDocument();
    expect(screen.getByText('都已授权，可以被远程控制了')).toBeInTheDocument();
  });

  it('opens the matching system settings pane for each missing permission', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    const { platform, user } = renderPage(<PermissionsSection />, { ipc });
    await flush();

    expect(within(permissionRow('screen_recording')).getByText('未授权')).toBeInTheDocument();
    expect(within(permissionRow('accessibility')).getByText('未授权')).toBeInTheDocument();
    expect(screen.getByText('授权后 macOS 可能会要求重新打开 Remote Host')).toBeInTheDocument();

    await user.click(
      within(permissionRow('screen_recording')).getByRole('button', { name: '打开系统设置' }),
    );
    await user.click(
      within(permissionRow('accessibility')).getByRole('button', { name: '打开系统设置' }),
    );

    expect(platform.calls).toContainEqual({
      op: 'openUrl',
      args: [SYSTEM_SETTINGS_URLS.screenRecording],
    });
    expect(platform.calls).toContainEqual({
      op: 'openUrl',
      args: [SYSTEM_SETTINGS_URLS.accessibility],
    });
  });

  it('refreshes when the permissions changed event arrives', async () => {
    const ipc = FakeIpc.fromScenario('setup');
    renderPage(<PermissionsSection />, { ipc });
    await flush();

    expect(within(permissionRow('screen_recording')).getByText('未授权')).toBeInTheDocument();

    act(() => {
      ipc.state.status.permissions.screen_recording = true;
      ipc.emit('event.permissionsChanged', {});
    });
    await flush();

    expect(within(permissionRow('screen_recording')).getByText('已授权')).toBeInTheDocument();
    expect(within(permissionRow('accessibility')).getByText('未授权')).toBeInTheDocument();
  });

  it('lists additional boolean permissions without actions', async () => {
    const ipc = FakeIpc.fromScenario('controlled');
    ipc.state.status.permissions.microphone = false;
    renderPage(<PermissionsSection />, { ipc });
    await flush();

    expect(screen.getByText('microphone：未授权')).toBeInTheDocument();
    expect(screen.queryAllByRole('button', { name: '打开系统设置' })).toHaveLength(0);
  });

  it('reopens the first-run setup window', async () => {
    const { platform, user } = renderPage(<PermissionsSection />);
    await flush();

    await user.click(screen.getByRole('button', { name: '重新运行首次设置…' }));

    expect(platform.calls).toContainEqual({
      op: 'showWindow',
      args: ['setup', 'setup'],
    });
  });
});
