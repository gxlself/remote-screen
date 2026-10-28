import { afterEach, describe, expect, it, vi } from 'vitest';

import { FakeIpc } from './fake';
import { APPROVAL_PAIRING } from './fixtures';
import { SCENARIOS } from './scenarios';

const NOW_MS = 1_792_465_134_000;

function createFake(name: string | null): FakeIpc {
  return FakeIpc.fromScenario(name, { now: () => NOW_MS });
}

describe('scenarios', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('builds every named scenario', () => {
    for (const name of SCENARIOS) {
      expect(createFake(name)).toBeInstanceOf(FakeIpc);
    }
  });

  it('keeps the controlled fixture unchanged', () => {
    const fake = createFake('controlled');

    expect(fake.state.sessions).toHaveLength(1);
    expect(fake.state.devices).toHaveLength(3);
    expect(fake.state.update).toEqual({ state: 'idle', version: null, notes: null });
  });

  it('clears sessions in idle', () => {
    expect(createFake('idle').state.sessions).toEqual([]);
  });

  it('adds the iPad viewer in handoff', () => {
    const fake = createFake('handoff');

    expect(fake.state.sessions).toHaveLength(2);
    expect(fake.state.sessions[1]).toEqual({
      session_id: 's-2',
      device_id: 'm4ntq8z2c7v5xa1d',
      device_name: '小明的 iPad',
      role: 'viewer',
      connected_at_us: (NOW_MS - 600_000) * 1000,
    });
  });

  it('emits a pairing approval 300 ms after building approval', async () => {
    vi.useFakeTimers();
    const fake = createFake('approval');
    const listener = vi.fn();

    expect(fake.state.sessions).toEqual([]);
    fake.subscribe(listener);
    await vi.advanceTimersByTimeAsync(299);
    expect(listener).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(listener).toHaveBeenCalledWith('event.approvalRequested', APPROVAL_PAIRING);
  });

  it('describes a pending update for update', () => {
    const fake = createFake('update');

    expect(fake.state.sessions).toHaveLength(1);
    expect(fake.state.update).toEqual({
      state: 'waiting_session_end',
      version: '0.3.2',
      notes: '修复：TCP 模式下偶发花屏\n新增：「内网连接每次确认」',
    });
  });

  it('makes update-ready available immediately', () => {
    const fake = createFake('update-ready');

    expect(fake.state.sessions).toEqual([]);
    expect(fake.state.update).toEqual({
      state: 'idle',
      version: '0.3.2',
      notes: '修复：TCP 模式下偶发花屏\n新增：「内网连接每次确认」',
    });
  });

  it('removes permissions, devices and sessions in setup', () => {
    const fake = createFake('setup');

    expect(fake.state.status.permissions).toEqual({
      screen_recording: false,
      accessibility: false,
    });
    expect(fake.state.devices).toEqual([]);
    expect(fake.state.sessions).toEqual([]);
  });

  it('starts offline disconnected', async () => {
    const fake = createFake('offline');

    expect(await fake.isConnected()).toBe(false);
  });

  it('treats an unknown scenario name as controlled', () => {
    const fake = createFake('does-not-exist');

    expect(fake.state).toEqual(createFake('controlled').state);
    expect(fake.state.sessions).toHaveLength(1);
  });
});
