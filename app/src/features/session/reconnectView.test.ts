import { localIdleReconnect } from '../../i18n/zh/localIdleReconnect';
import type { DialAttempt } from '../../native/rscore/types';
import {
  formatReconnectClock,
  reconnectView,
  type ReconnectViewInput,
} from './reconnectView';

const copy = localIdleReconnect.reconnect;

function input(
  overrides: Partial<ReconnectViewInput> = {},
): ReconnectViewInput {
  return {
    reason: 'pathLost',
    os: 'macos',
    dial: [],
    sinceMs: 0,
    nowMs: 0,
    netChangedRecently: false,
    ...overrides,
  };
}

describe('reconnectView', () => {
  it('formats the paused clock as local HH:mm:ss', () => {
    expect(formatReconnectClock(Date.UTC(2026, 9, 21, 2, 24, 31))).toBe(
      '02:24:31',
    );
  });

  it.each([
    ['host updating by reconnect reason', { reason: 'hostUpdating' as const }],
    [
      'host updating by goodbye reason',
      { reason: null, endedReason: 'hostUpdating' as const },
    ],
  ])('uses the host-updating copy for %s', (_name, overrides) => {
    const view = reconnectView(input(overrides));
    expect(view.title).toBe('电脑正在更新');
    expect(view.desc).toBe('Remote Host 正在更新，装好后自动接回会话');
    expect(view.showTerminal).toBe(false);
  });

  it('uses the resume copy', () => {
    const view = reconnectView(input({ reason: 'resume' }));
    expect(view.title).toBe('正在恢复会话');
    expect(view.desc).toBe('App 回到前台，正在接回会话。');
  });

  it('uses the network-changed copy when the network changed recently', () => {
    const view = reconnectView(input({ netChangedRecently: true }));
    expect(view.title).toBe('正在重新连接');
    expect(view.desc).toBe('手机的网络变了。会话还在 Mac 上保持着。');
  });

  it('uses the path-lost copy by default', () => {
    const view = reconnectView(input());
    expect(view.title).toBe('正在重新连接');
    expect(view.desc).toBe('连接断了一下。会话还在 Mac 上保持着。');
  });

  it('marks the tunnel done with path names and advances the host step', () => {
    const dial: DialAttempt[] = [
      { path: 'tailnetDerp', addr: '100.64.0.7:7400', state: 'ok' },
    ];
    const view = reconnectView(input({ dial }));
    expect(view.steps[0]).toMatchObject({
      label: '网络已接通',
      state: 'done',
      detail: '· 中继',
    });
    expect(view.steps[0].detail).toContain('中继');
    expect(view.steps[1]).toMatchObject({
      label: '正在等电脑接回会话',
      state: 'active',
    });
    expect(view.steps[2]).toMatchObject({
      label: copy.steps.frame.pending,
      state: 'pending',
    });
  });

  it('keeps the tunnel active and the host pending without an ok dial', () => {
    const dial: DialAttempt[] = [
      { path: 'tailnetDirect', addr: '100.64.0.7:7400', state: 'failed' },
      { path: 'intranet', addr: '10.30.12.48:7400', state: 'trying' },
    ];
    const view = reconnectView(input({ dial }));
    // R12：还在转圈时不能说「已恢复」。
    expect(view.steps[0]).toMatchObject({
      label: '正在接通网络',
      state: 'active',
      detail: null,
    });
    expect(view.steps[1]).toMatchObject({ state: 'pending' });
    expect(view.steps[2]).toMatchObject({ state: 'pending' });
  });

  it('formats the elapsed wait in whole seconds', () => {
    const view = reconnectView(input({ sinceMs: 0, nowMs: 2400 }));
    expect(view.waitedText).toBe(
      '已等 2 秒 · 60 秒内恢复不需要重新授权。重连期间的触摸和按键不会发出，避免误操作。',
    );
  });

  it('offers the terminal only when the computer has an SSH server', () => {
    expect(reconnectView(input({ hasTerminal: false })).showTerminal).toBe(
      false,
    );
    expect(reconnectView(input({ hasTerminal: true })).showTerminal).toBe(true);
  });

  it('avoids tunnel and keyframe jargon', () => {
    const view = reconnectView(input());
    const text = view.steps.map(step => step.label).join(' ');
    expect(text).not.toMatch(/tailnet|隧道|关键帧|Host/);
  });

  it('does not mention Mac on a Windows host', () => {
    const lost = reconnectView(input({ os: 'windows' }));
    const changed = reconnectView(
      input({ os: 'windows', netChangedRecently: true }),
    );
    expect(lost.desc).not.toContain('Mac');
    expect(changed.desc).not.toContain('Mac');
    expect(lost.desc).toContain('电脑');
  });
});
