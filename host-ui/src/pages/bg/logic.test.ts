import { describe, expect, it } from 'vitest';

import type { SessionSummary } from '../../ipc/types';

import { diffSessions, trayStateOf } from './logic';

function session(
  sessionId: string,
  deviceName: string,
  role: SessionSummary['role'],
): SessionSummary {
  return {
    session_id: sessionId,
    device_id: `device-${sessionId}`,
    device_name: deviceName,
    role,
    connected_at_us: 1_000_000,
  };
}

const IPHONE_CONTROLLER = session('s-1', '小明的 iPhone', 'controller');
const IPAD_VIEWER = session('s-2', '小明的 iPad', 'viewer');

describe('trayStateOf', () => {
  it('prioritizes connection, sessions, then a pending update', () => {
    expect(trayStateOf({ connected: false, sessions: 2, updateVersion: '0.3.2' })).toBe('offline');
    expect(trayStateOf({ connected: true, sessions: 1, updateVersion: '0.3.2' })).toBe(
      'controlled',
    );
    expect(trayStateOf({ connected: true, sessions: 0, updateVersion: '0.3.2' })).toBe('update');
    expect(trayStateOf({ connected: true, sessions: 0, updateVersion: null })).toBe('idle');
  });
});

describe('diffSessions', () => {
  it('does not notify on the first snapshot', () => {
    expect(diffSessions(null, [IPHONE_CONTROLLER])).toEqual([]);
  });

  it('notifies when a viewer joins a controlled session', () => {
    expect(diffSessions([IPHONE_CONTROLLER], [IPHONE_CONTROLLER, IPAD_VIEWER])).toEqual([
      {
        title: '小明的 iPad 加入了，正在观看',
        body: '小明的 iPhone 仍在控制这台 Mac。小明的 iPad 上点「接管控制」就能换过来。',
      },
    ]);
  });

  it('notifies when control moves to another device that was already viewing', () => {
    expect(
      diffSessions(
        [session('s-2', '小明的 iPad', 'controller'), IPHONE_CONTROLLER],
        [session('s-1', '小明的 iPhone', 'controller'), session('s-2', '小明的 iPad', 'viewer')],
      ),
    ).toEqual([
      {
        title: '小明的 iPhone 接管了控制',
        body: '小明的 iPad 现在只能观看。两台都能在菜单栏里单独断开。',
      },
    ]);
  });

  it('uses the short handoff body when the previous controller left', () => {
    expect(
      diffSessions(
        [session('s-2', '小明的 iPad', 'controller')],
        [session('s-1', '小明的 iPhone', 'controller')],
      ),
    ).toEqual([
      {
        title: '小明的 iPhone 接管了控制',
        body: '可以在菜单栏里断开。',
      },
    ]);
  });

  it('does not notify when sessions start or end', () => {
    expect(diffSessions([IPHONE_CONTROLLER], [])).toEqual([]);
    expect(diffSessions([], [IPHONE_CONTROLLER])).toEqual([]);
    expect(diffSessions([], [IPHONE_CONTROLLER, IPAD_VIEWER])).toEqual([]);
  });
});
