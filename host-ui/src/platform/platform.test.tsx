import { renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { FakePlatform } from './fake';
import { PlatformProvider, usePlatform } from './react';
import { createTauriPlatform } from './tauri';
import { SYSTEM_SETTINGS_URLS, WINDOW_LABELS } from './types';

const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

describe('FakePlatform', () => {
  it('defaults to macOS and accepts an explicit host OS', () => {
    expect(new FakePlatform().os).toBe('macos');
    expect(new FakePlatform({ os: 'windows' }).os).toBe('windows');
  });

  it('records window and clipboard calls', async () => {
    const platform = new FakePlatform();

    await platform.showWindow('pair', 'pair');
    await platform.copyText('x');

    expect(platform.calls).toEqual([
      { op: 'showWindow', args: ['pair', 'pair'] },
      { op: 'copyText', args: ['x'] },
    ]);
    expect(platform.clipboard).toBe('x');
  });
});

describe('createTauriPlatform', () => {
  it('maps every operation to its Tauri command', async () => {
    const platform = createTauriPlatform();

    await platform.showWindow('settings', 'settings/devices');
    await platform.showWindow('menu');
    await platform.hideWindow('pair');
    await platform.setTrayState('controlled');
    await platform.notify('Remote Host', 'paired');
    await platform.openUrl(SYSTEM_SETTINGS_URLS.accessibility);
    await platform.revealPath('/tmp/logs');
    await platform.quit();

    expect(invoke.mock.calls).toEqual([
      ['window_show', { label: 'settings', route: 'settings/devices' }],
      ['window_show', { label: 'menu', route: null }],
      ['window_hide', { label: 'pair' }],
      ['tray_set_state', { state: 'controlled' }],
      ['notify', { title: 'Remote Host', body: 'paired' }],
      ['open_url', { url: SYSTEM_SETTINGS_URLS.accessibility }],
      ['reveal_path', { path: '/tmp/logs' }],
      ['quit_app'],
    ]);
  });

  it('copies with the clipboard API', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });

    await createTauriPlatform().copyText('secret');

    expect(writeText).toHaveBeenCalledWith('secret');
  });

  it('reads the host OS from the platform detector', async () => {
    vi.stubGlobal('__RS_HOST_OS__', 'windows');
    expect(createTauriPlatform().os).toBe('windows');
  });
});

describe('usePlatform', () => {
  it('throws without a provider', () => {
    expect(() => renderHook(() => usePlatform())).toThrow(
      'usePlatform must be used inside a PlatformProvider',
    );
  });

  it('returns the provided platform', () => {
    const platform = new FakePlatform();
    const { result } = renderHook(() => usePlatform(), {
      wrapper: ({ children }) => (
        <PlatformProvider platform={platform}>{children}</PlatformProvider>
      ),
    });
    expect(result.current).toBe(platform);
  });
});

describe('WINDOW_LABELS', () => {
  it('lists every Tauri window', () => {
    expect(WINDOW_LABELS).toEqual([
      'bg',
      'menu',
      'capsule',
      'border',
      'confirm',
      'pair',
      'settings',
      'setup',
      'update',
    ]);
  });
});
