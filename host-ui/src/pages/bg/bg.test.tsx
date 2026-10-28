import { act, screen } from '@testing-library/react';
import { StrictMode } from 'react';
import { describe, expect, it } from 'vitest';

import { readFlag, SETUP_DONE_KEY } from '../../app/storage';
import { FakeIpc } from '../../ipc/fake';
import { flush, renderPage } from '../../ipc/testing';

import BgPage from '.';

function renderBg(ipc: FakeIpc) {
  return renderPage(
    <StrictMode>
      <BgPage route={{ page: 'bg', query: {} }} />
    </StrictMode>,
    { ipc },
  );
}

function platformCalls(calls: ReturnType<typeof renderBg>['platform']['calls'], op: string) {
  return calls.filter((call) => call.op === op);
}

describe('BgPage orchestration', () => {
  it('shows the capsule and border for an existing controlled session', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);

    await flush();

    expect(platformCalls(platform.calls, 'showWindow')).toEqual([
      { op: 'showWindow', args: ['capsule', undefined] },
      { op: 'showWindow', args: ['border', undefined] },
    ]);
    expect(platformCalls(platform.calls, 'setTrayState')).toEqual([
      { op: 'setTrayState', args: ['controlled'] },
    ]);
    expect(platformCalls(platform.calls, 'showWindow')).not.toContainEqual({
      op: 'showWindow',
      args: ['setup', 'setup'],
    });
  });

  it('hides the capsule and border after the last session disconnects only once', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);
    await flush();

    await act(async () => {
      await ipc.call('sessions.disconnect', { session_id: 's-1' });
    });
    await flush();

    expect(platformCalls(platform.calls, 'hideWindow')).toEqual([
      { op: 'hideWindow', args: ['capsule'] },
      { op: 'hideWindow', args: ['border'] },
    ]);
    expect(platform.calls.filter((call) => call.op === 'setTrayState').at(-1)).toEqual({
      op: 'setTrayState',
      args: ['idle'],
    });

    const before = platform.calls.length;
    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();

    expect(platform.calls.slice(before)).toEqual([]);
  });

  it('opens the setup guide once on first successful start', async () => {
    const ipc = FakeIpc.fromScenario('idle');
    const { platform } = renderBg(ipc);
    await flush();

    expect(platformCalls(platform.calls, 'showWindow')).toEqual([
      { op: 'showWindow', args: ['setup', 'setup'] },
    ]);
    expect(platformCalls(platform.calls, 'setTrayState')).toEqual([
      { op: 'setTrayState', args: ['idle'] },
    ]);
    expect(readFlag(SETUP_DONE_KEY)).toBeNull();
  });

  it('shows the update tray state when an update is ready', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('update-ready');
    const { platform } = renderBg(ipc);
    await flush();

    expect(platformCalls(platform.calls, 'setTrayState')).toEqual([
      { op: 'setTrayState', args: ['update'] },
    ]);
  });

  it('notifies once when a viewer joins', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);
    await flush();

    ipc.state.sessions.push({
      session_id: 's-2',
      device_id: 'm4ntq8z2c7v5xa1d',
      device_name: '小明的 iPad',
      role: 'viewer',
      connected_at_us: 1_000_000,
    });
    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();

    expect(platformCalls(platform.calls, 'notify')).toEqual([
      {
        op: 'notify',
        args: [
          '小明的 iPad 加入了，正在观看',
          '小明的 iPhone 仍在控制这台 Mac。小明的 iPad 上点「接管控制」就能换过来。',
        ],
      },
    ]);
  });

  it('notifies once when control is handed off to the viewer', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('handoff');
    const { platform } = renderBg(ipc);
    await flush();

    const iphone = ipc.state.sessions.find((session) => session.session_id === 's-1');
    const ipad = ipc.state.sessions.find((session) => session.session_id === 's-2');
    expect(iphone).toBeDefined();
    expect(ipad).toBeDefined();
    if (iphone !== undefined && ipad !== undefined) {
      iphone.role = 'viewer';
      ipad.role = 'controller';
    }
    act(() => ipc.emit('event.handoff', {}));
    await flush();

    expect(platformCalls(platform.calls, 'notify')).toEqual([
      {
        op: 'notify',
        args: [
          '小明的 iPad 接管了控制',
          '小明的 iPhone 现在只能观看。两台都能在菜单栏里单独断开。',
        ],
      },
    ]);
  });

  it('notifies on a successful pairing only', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);
    await flush();

    act(() =>
      ipc.emit('event.pairingResult', {
        status: 1,
        host_id: 'h7v2m9q3k8x4tb1a',
        host_name: 'studio-mac',
        attempts_left: 0,
        device_fp: '',
      }),
    );
    act(() =>
      ipc.emit('event.pairingResult', {
        status: 4,
        host_id: 'h7v2m9q3k8x4tb1a',
        host_name: 'studio-mac',
        attempts_left: 0,
        device_fp: '',
      }),
    );
    await flush();

    expect(platformCalls(platform.calls, 'notify')).toEqual([
      {
        op: 'notify',
        args: ['新设备配对成功', '可以在菜单栏 › 已配对的设备里修改它的权限。'],
      },
    ]);
  });

  it('uses a distinct title for each successful pairing method', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);
    await flush();

    const base = {
      status: 1,
      host_id: 'h7v2m9q3k8x4tb1a',
      host_name: 'studio-mac',
      attempts_left: 0,
      device_fp: '',
    };

    act(() => ipc.emit('event.pairingResult', { ...base, method: 'code' }));
    act(() => ipc.emit('event.pairingResult', { ...base, method: 'file' }));
    act(() =>
      ipc.emit('event.pairingResult', {
        ...base,
        method: 'password',
        device_name: '小明的 iPhone',
        account: 'alice',
      }),
    );
    act(() =>
      ipc.emit('event.pairingResult', {
        ...base,
        method: 'approve',
        device_name: '小明的 iPad',
      }),
    );
    await flush();

    expect(platformCalls(platform.calls, 'notify').map((call) => call.args)).toEqual([
      ['新设备配对成功', '可以在菜单栏 › 已配对的设备里修改它的权限。'],
      ['新设备用配对文件完成了配对', '可以在菜单栏 › 已配对的设备里修改它的权限。'],
      ['新设备用账户 alice 完成了配对', '可以在菜单栏 › 已配对的设备里修改它的权限。'],
      ['小明的 iPad 经你的其他设备批准加入', '可以在菜单栏 › 已配对的设备里修改它的权限。'],
    ]);
  });

  it('omits the account when password pairing has no account', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);
    await flush();

    act(() =>
      ipc.emit('event.pairingResult', {
        status: 1,
        host_id: 'h7v2m9q3k8x4tb1a',
        host_name: 'studio-mac',
        attempts_left: 0,
        device_fp: '',
        method: 'password',
      }),
    );
    act(() =>
      ipc.emit('event.pairingResult', {
        status: 4,
        host_id: 'h7v2m9q3k8x4tb1a',
        host_name: 'studio-mac',
        attempts_left: 4,
        device_fp: '',
        method: 'file',
      }),
    );
    await flush();

    expect(platformCalls(platform.calls, 'notify').map((call) => call.args)).toEqual([
      ['新设备用账户完成了配对', '可以在菜单栏 › 已配对的设备里修改它的权限。'],
    ]);
  });

  it('reflects disconnects and reconnects without duplicate effects', async () => {
    localStorage.setItem(SETUP_DONE_KEY, '1');
    const ipc = FakeIpc.fromScenario('controlled');
    const { platform } = renderBg(ipc);
    await flush();

    act(() => ipc.setConnected(false));
    await flush();
    expect(platform.calls.filter((call) => call.op === 'setTrayState').at(-1)).toEqual({
      op: 'setTrayState',
      args: ['offline'],
    });
    expect(platformCalls(platform.calls, 'hideWindow')).toEqual([
      { op: 'hideWindow', args: ['capsule'] },
      { op: 'hideWindow', args: ['border'] },
    ]);

    act(() => ipc.setConnected(true));
    await flush();
    expect(platform.calls.filter((call) => call.op === 'setTrayState').at(-1)).toEqual({
      op: 'setTrayState',
      args: ['controlled'],
    });
    expect(platformCalls(platform.calls, 'showWindow')).toEqual([
      { op: 'showWindow', args: ['capsule', undefined] },
      { op: 'showWindow', args: ['border', undefined] },
      { op: 'showWindow', args: ['capsule', undefined] },
      { op: 'showWindow', args: ['border', undefined] },
    ]);
  });

  it('renders a hidden background page', () => {
    const ipc = FakeIpc.fromScenario('controlled');
    renderBg(ipc);

    const page = document.querySelector('[data-page="bg"]');
    expect(page).not.toBeNull();
    expect(page).toHaveAttribute('hidden');
    expect(screen.queryByText('（待实现）')).not.toBeInTheDocument();
  });
});
