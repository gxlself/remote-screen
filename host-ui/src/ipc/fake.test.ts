import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FakeIpc } from './fake';
import { APPROVAL_CONNECTION, APPROVAL_PAIRING, DEVICE_FP_B64, HOST_FP_B64 } from './fixtures';
import type { RpcErrorShape } from './transport';
import type { AuditListResult } from './types';

const NOW_MS = 1_792_465_134_000;

function fakeNow(): () => number {
  return () => NOW_MS;
}

async function captureRejection(promise: Promise<unknown>): Promise<RpcErrorShape> {
  let caught: unknown;
  try {
    await promise;
  } catch (error) {
    caught = error;
  }
  expect(caught).toBeTypeOf('object');
  return caught as RpcErrorShape;
}

describe('FakeIpc', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('exports the fixture fingerprints as unpadded base64url', () => {
    expect(HOST_FP_B64).toBe('fzqRwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAF4rwtQ');
    expect(DEVICE_FP_B64).toBe('nE4begAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAANA_Yqg');
    expect(HOST_FP_B64).not.toContain('=');
    expect(DEVICE_FP_B64).not.toContain('=');
  });

  it('returns status with the current sessions', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const status = await fake.call('status.get', null);

    expect(status).toEqual({
      online: true,
      tailnet_addresses: ['100.64.0.7'],
      version: '0.3.1',
      permissions: { screen_recording: true, accessibility: true },
      listening: true,
      sessions: [
        {
          session_id: 's-1',
          device_id: 'k3jq7x2m9p4tbw8a',
          device_name: '小明的 iPhone',
          role: 'controller',
          connected_at_us: (NOW_MS - 2_538_000) * 1000,
        },
      ],
    });
    expect(fake.calls).toEqual([{ method: 'status.get', params: null }]);
  });

  it('accepts an empty object for no-argument methods', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    await expect(fake.call('status.get', {})).resolves.toMatchObject({ online: true });
    await expect(fake.call('status.get', { unexpected: true })).rejects.toEqual({
      code: -32602,
      message: 'invalid params',
    });
  });

  it('disconnects a session and notifies subscribers once', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const listener = vi.fn();
    fake.subscribe(listener);

    await fake.call('sessions.disconnect', { session_id: 's-1' });

    await expect(fake.call('sessions.list', null)).resolves.toEqual({ sessions: [] });
    expect(listener).toHaveBeenCalledTimes(1);
    expect(listener).toHaveBeenCalledWith('event.sessionsChanged', { sessions: [] });
  });

  it('rejects disconnecting an unknown session with not_found', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const error = await captureRejection(
      fake.call('sessions.disconnect', { session_id: 'missing' }),
    );

    expect(error).toEqual({
      code: -32000,
      message: 'session not found',
      data: { code: 'not_found' },
    });
  });

  it('changes a session role to viewer and emits sessionsChanged', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const listener = vi.fn();
    fake.subscribe(listener);

    await fake.call('sessions.setViewOnly', { session_id: 's-1' });

    expect(fake.state.sessions[0]?.role).toBe('viewer');
    expect(listener).toHaveBeenCalledWith('event.sessionsChanged', {
      sessions: [expect.objectContaining({ session_id: 's-1', role: 'viewer' })],
    });
  });

  it('cycles pairing codes and builds the QR URL with an injected clock', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const first = await fake.call('pairing.start', null);
    const second = await fake.call('pairing.start', null);

    expect(first).toMatchObject({ code: '482913', expires_at: (NOW_MS + 300_000) * 1000 });
    expect(second).toMatchObject({ code: '173064', expires_at: (NOW_MS + 300_000) * 1000 });
    expect(first).toMatchObject({
      qr_url:
        `rsp://pair?v=1&h=h7v2m9q3k8x4tb1a&fp=${HOST_FP_B64}&c=482913` +
        '&n=studio-mac&a=100.64.0.7:7400',
    });
    expect(fake.state.pairing).toEqual(second);
  });

  it('updates device permissions and paths', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    await fake.call('devices.update', {
      device_id: 'k3jq7x2m9p4tbw8a',
      permission: 'control',
      paths: ['intranet'],
    });

    expect(fake.state.devices[0]).toMatchObject({ permission: 'control', paths: ['intranet'] });
  });

  it('rejects an invalid device permission with -32602', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const error = await captureRejection(
      fake.call('devices.update', {
        device_id: 'k3jq7x2m9p4tbw8a',
        permission: 'admin',
        paths: [],
      }),
    );

    expect(error).toEqual({ code: -32602, message: 'invalid params' });
  });

  it('deep merges settings and replaces arrays wholesale', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const originalClipboard = structuredClone(fake.state.settings.clipboard);

    const settings = await fake.call('settings.set', { access: { lock_on_end: true } });

    expect(settings).toEqual({
      ...fake.state.settings,
      access: { ...fake.state.settings.access, lock_on_end: true },
    });
    expect(fake.state.settings.clipboard).toEqual(originalClipboard);

    await fake.call('settings.set', { intranet: { allow_sources: ['10.30.12.0/24'] } });

    expect(fake.state.settings.intranet.allow_sources).toEqual(['10.30.12.0/24']);
    expect(fake.state.settings.intranet.interfaces).toEqual(['en0']);
  });

  it('provides defaults and persists the password pairing security keys', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    expect(fake.state.settings.security).toEqual({
      confirm_new_devices: true,
      password_pairing: false,
      password_pairing_permission: 'control',
    });

    await fake.call('settings.set', {
      security: { password_pairing: true, password_pairing_permission: 'view_only' },
    });

    expect(fake.state.settings.security).toMatchObject({
      password_pairing: true,
      password_pairing_permission: 'view_only',
    });
  });

  it('exports, lists and revokes pairing files', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const exported = (await fake.call('pairing.exportFile', {
      permission: 'view_only',
      label: '现场支持',
    })) as { file_id: string; path: string; file_name: string };
    expect(exported.path).toBe(`/Users/example/Downloads/${exported.file_name}`);
    expect(exported.file_name).toMatch(/\.rspair$/);

    const listed = (await fake.call('pairing.listFiles', null)) as {
      files: Array<{ file_id: string; state: string }>;
    };
    expect(listed.files.map((file) => file.state)).toEqual(['active', 'active', 'used']);
    expect(listed.files[0]?.file_id).toBe(exported.file_id);

    await fake.call('pairing.revokeFile', { file_id: exported.file_id });
    const after = (await fake.call('pairing.listFiles', null)) as {
      files: Array<{ file_id: string; state: string }>;
    };
    expect(after.files[0]).toMatchObject({ file_id: exported.file_id, state: 'revoked' });
  });

  it('rejects non-object settings patches with -32602', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    await expect(fake.call('settings.set', [])).rejects.toEqual({
      code: -32602,
      message: 'invalid params',
    });
    await expect(fake.call('settings.set', 'invalid')).rejects.toEqual({
      code: -32602,
      message: 'invalid params',
    });
  });

  it('lists latest audit entries and filters by before_us', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const latest = (await fake.call('audit.list', { limit: 1 })) as AuditListResult;
    expect(latest.entries).toHaveLength(1);
    expect(latest.entries[0]).toMatchObject({ event: 'session.end', duration_s: 2538 });

    const before = (await fake.call('audit.list', {
      before_us: (NOW_MS - 2 * 60 * 60 * 1000) * 1000,
    })) as AuditListResult;
    expect(before.entries).toHaveLength(2);
    expect(before.entries[0]).toMatchObject({
      event: 'session.start',
      device_id: 'k3jq7x2m9p4tbw8a',
    });
    expect(before.entries[1]).toMatchObject({
      event: 'session.start',
      device_id: 'm4ntq8z2c7v5xa1d',
    });
  });

  it('rejects unknown methods with -32601', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const error = await captureRejection(fake.call('unknown.method', null));

    expect(error).toEqual({ code: -32601, message: 'method not found' });
  });

  it('applies failNext only to the next matching call', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const failure = { code: -32000, message: 'busy', data: { code: 'busy' } };

    fake.failNext('status.get', failure);

    await expect(fake.call('status.get', null)).rejects.toEqual(failure);
    await expect(fake.call('status.get', null)).resolves.toMatchObject({ online: true });
  });

  it('rejects calls while disconnected and notifies on connection changes', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const connections = vi.fn();
    fake.onConnection(connections);

    fake.setConnected(false);

    await expect(fake.call('status.get', null)).rejects.toMatchObject({
      code: -32099,
      data: { code: 'agent_unavailable' },
    });
    expect(await fake.isConnected()).toBe(false);
    expect(connections).toHaveBeenCalledTimes(1);
    expect(connections).toHaveBeenCalledWith(false);
  });

  it('returns copies so mutating a result cannot affect state', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });

    const devices = (await fake.call('devices.list', null)) as {
      devices: Array<{ name: string; paths: string[] }>;
    };
    devices.devices[0]!.name = 'changed';
    devices.devices[0]!.paths.push('intranet');

    expect(fake.state.devices[0]?.name).toBe('小明的 iPhone');
    expect(fake.state.devices[0]?.paths).toEqual(['tailnet', 'lan']);
  });

  it('honours a positive delay before resolving', async () => {
    vi.useFakeTimers();
    const fake = new FakeIpc(undefined, {
      now: fakeNow(),
      delay: (method) => (method === 'status.get' ? 50 : 0),
    });
    let resolved = false;

    const pending = fake.call('status.get', null).then(() => {
      resolved = true;
    });
    await vi.advanceTimersByTimeAsync(49);
    expect(resolved).toBe(false);

    await vi.advanceTimersByTimeAsync(1);
    await pending;
    expect(resolved).toBe(true);
  });

  it('closes pairing approvals when pairing is cancelled', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const listener = vi.fn();
    fake.subscribe(listener);
    fake.emit('event.approvalRequested', APPROVAL_PAIRING);

    await fake.call('pairing.cancel', null);

    expect(fake.state.pairing).toBeNull();
    expect(listener).toHaveBeenCalledWith('event.approvalClosed', {
      request_id: 'r-1',
      kind: 'pairing',
      outcome: 'cancelled',
    });
  });

  it('records approval responses and emits approvalClosed', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const listener = vi.fn();
    fake.subscribe(listener);
    fake.emit('event.approvalRequested', APPROVAL_CONNECTION);

    await fake.call('approval.respond', { request_id: 'r-2', allow: true });

    expect(fake.state.approvals).toEqual([{ request_id: 'r-2', allow: true }]);
    expect(listener).toHaveBeenCalledWith('event.approvalClosed', {
      request_id: 'r-2',
      kind: 'connection',
      outcome: 'allowed',
    });
  });

  it('lists requested approvals in arrival order until they close or are answered', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    fake.emit('event.approvalRequested', APPROVAL_PAIRING);
    fake.emit('event.approvalRequested', APPROVAL_CONNECTION);

    await expect(fake.call('approval.pending', null)).resolves.toEqual({
      requests: [APPROVAL_PAIRING, APPROVAL_CONNECTION],
    });

    await fake.call('approval.respond', { request_id: 'r-1', allow: true });
    fake.emit('event.approvalClosed', {
      request_id: 'r-2',
      kind: 'connection',
      outcome: 'cancelled',
    });

    await expect(fake.call('approval.pending', null)).resolves.toEqual({ requests: [] });
  });

  it('returns pending approvals as copies and rejects unexpected params', async () => {
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    fake.emit('event.approvalRequested', APPROVAL_PAIRING);

    const result = (await fake.call('approval.pending', null)) as {
      requests: Array<{ device_name: string }>;
    };
    result.requests[0]!.device_name = 'changed';

    expect(await fake.call('approval.pending', null)).toMatchObject({
      requests: [{ request_id: 'r-1', device_name: '小明的 iPhone' }],
    });
    await expect(fake.call('approval.pending', { unexpected: true })).rejects.toEqual({
      code: -32602,
      message: 'invalid params',
    });
  });

  it('times out a pending approval using its payload timeout_s', async () => {
    vi.useFakeTimers();
    const fake = new FakeIpc(undefined, { now: fakeNow() });
    const listener = vi.fn();
    fake.subscribe(listener);
    fake.emit('event.approvalRequested', { ...APPROVAL_CONNECTION, timeout_s: 5 });

    await vi.advanceTimersByTimeAsync(4_999);
    expect(listener).not.toHaveBeenCalledWith('event.approvalClosed', expect.anything());

    await vi.advanceTimersByTimeAsync(1);
    expect(listener).toHaveBeenCalledWith('event.approvalClosed', {
      request_id: 'r-2',
      kind: 'connection',
      outcome: 'timeout',
    });
  });
});
