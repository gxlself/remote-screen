import { afterEach, describe, expect, it, vi } from 'vitest';

import { detectHostOs } from '../platform/os';
import { zh as zhBase } from './zh';
import { zhFor } from './winText';

interface TranslationEntry {
  path: string;
  value: unknown;
}

interface HostOsGlobal {
  __RS_HOST_OS__?: unknown;
}

function collectEntries(value: unknown, path: string, entries: TranslationEntry[]): void {
  if (typeof value === 'string' || typeof value === 'function') {
    entries.push({ path, value });
    return;
  }
  if (typeof value !== 'object' || value === null) {
    return;
  }
  for (const [key, child] of Object.entries(value)) {
    collectEntries(child, path.length === 0 ? key : `${path}.${key}`, entries);
  }
}

function sampleArgument(path: string): unknown {
  if (
    path.endsWith('.controlling') ||
    path.endsWith('.watching') ||
    path.endsWith('.watchingOne') ||
    path.endsWith('.viewerOne')
  ) {
    return '小明的 iPhone';
  }
  if (path === 'confirm.description.takeover') {
    return null;
  }
  if (
    path.endsWith('.watchingMany') ||
    path.endsWith('.viewerMany') ||
    path.endsWith('.countdown') ||
    path.endsWith('.queued')
  ) {
    return 2;
  }
  return '小明的 iPhone';
}

afterEach(() => {
  delete (globalThis as HostOsGlobal).__RS_HOST_OS__;
  vi.unstubAllGlobals();
});

describe('zhFor', () => {
  it('returns the base translations by reference on macOS', () => {
    expect(zhFor('macos')).toBe(zhBase);
  });

  it('removes Mac-only wording from every Windows namespace except setup', () => {
    const windows = zhFor('windows');
    const entries: TranslationEntry[] = [];
    collectEntries(windows, '', entries);

    const checked = entries.filter(({ path }) => !path.startsWith('setup.'));
    expect(checked.length).toBeGreaterThan(0);
    for (const { path, value } of checked) {
      const rendered =
        typeof value === 'function'
          ? String((value as (...args: unknown[]) => unknown)(sampleArgument(path)))
          : String(value);
      expect(rendered, path).not.toMatch(/Mac|macOS|⌘/);
    }
  });

  it('uses the WinTray and WinConfirm copy from the canvas', () => {
    const windows = zhFor('windows');

    expect(windows.menu.online).toBe('服务运行中');
    expect(windows.menu.offline).toBe('服务没有运行');
    expect(windows.capsule.controlling('小明的 iPhone')).toBe('小明的 iPhone 正在控制此电脑');
    expect(windows.capsule.watchingOne('小明的 iPhone')).toBe('小明的 iPhone 正在观看此电脑');
    expect(windows.capsule.watchingMany(2)).toBe('2 台设备正在观看此电脑');
    expect(windows.confirm.description.intranet).toBe(
      '这次是从公司内网连进来的，不经过 tailnet。确认是你本人再允许。',
    );
    expect(windows.network.intranet.firewallActive).toBe(
      '已添加 Windows 防火墙规则（域、专用网络）',
    );
    expect(windows.network.intranet.firewallOff).toBe('Windows 防火墙没有打开，不需要规则');
    expect(windows.network.alerts.noRoutes).toBe(
      '两条路都关了，手机现在连不上这台电脑。至少打开一个。',
    );
    expect(windows.pairingMethods.password.switchLabel).toBe('允许用 Windows 账户密码配对');
    expect(windows.pairingMethods.password.hint).toBe(
      '适合没人能看配对码的时候，错 5 次锁 15 分钟',
    );
  });
});

describe('detectHostOs', () => {
  it('uses the devtools override', () => {
    (globalThis as HostOsGlobal).__RS_HOST_OS__ = 'windows';
    expect(detectHostOs()).toBe('windows');
  });

  it('detects Windows from the user agent', () => {
    vi.stubGlobal('navigator', {
      userAgent:
        'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0 Safari/537.36',
    });
    expect(detectHostOs()).toBe('windows');
  });

  it('defaults to macOS in jsdom', () => {
    expect(detectHostOs()).toBe('macos');
  });
});
