import { describe, expect, it, vi } from 'vitest';

import type { CoreErrorLike } from '@app/native/rscore/bindings';
import { createRsCore } from '@app/native/rscore/createRsCore';
import { RsCoreError } from '@app/native/rscore/RsCoreApi';

import { desktopBindings } from './bindings';
import { fakeTransport, type FakeOptions } from './fake';

function setup(opts?: FakeOptions) {
  const t = fakeTransport(opts);
  const b = desktopBindings(t);
  return { t, b, core: createRsCore(b) };
}

function caught(fn: () => unknown): unknown {
  try {
    fn();
  } catch (e) {
    return e;
  }
  throw new Error('expected a throw');
}

describe('desktopBindings', () => {
  it('bindings.args', async () => {
    const { t, b } = setup();

    b.sessionsClose(5n);
    await b.diagProbeAddress('192.168.31.5', undefined);
    b.sessionsSelectDisplay(5n, 2);
    b.sessionsAction(5n, 'lock', undefined);

    expect(t.calls).toEqual([
      { cmd: 'rscore_call', args: { name: 'sessionsClose', args: ['5'] } },
      { cmd: 'rscore_call', args: { name: 'diagProbeAddress', args: ['192.168.31.5', null] } },
      { cmd: 'rscore_call', args: { name: 'sessionsSelectDisplay', args: ['5', 2] } },
      { cmd: 'rscore_call', args: { name: 'sessionsAction', args: ['5', 'lock', null] } },
    ]);
  });

  it('bindings.bigint_return', async () => {
    const { b, core } = setup({ rscore: { sessionsOpen: () => '42', diagProbe: () => '7' } });

    await expect(b.sessionsOpen('h7v2m9q3k8x4tb1a', '{}')).resolves.toBe(42n);
    await expect(b.diagProbe('h7v2m9q3k8x4tb1a')).resolves.toBe(7n);
    await expect(core.sessions.open('h7v2m9q3k8x4tb1a', { intent: 'control' })).resolves.toBe('42');
  });

  it('bindings.string_and_optional_returns', async () => {
    const { b, core } = setup({ rscore: { hostsForget: () => null } });

    await expect(b.hostsForget('h7v2m9q3k8x4tb1a')).resolves.toBeUndefined();
    await expect(core.hosts.forget('h7v2m9q3k8x4tb1a')).resolves.toBeUndefined();
    await expect(core.identity.ensure('电脑')).resolves.toHaveProperty('deviceId');
  });

  it('bindings.error_passthrough', async () => {
    const rejection: CoreErrorLike = {
      tag: 'Http',
      inner: { message: 'm', status: 403, hubCode: 'forbidden' },
    };
    const { b, core } = setup({
      rscore: {
        hostsPaired: () => {
          throw rejection;
        },
      },
    });

    await expect(b.hostsPaired()).rejects.toBe(rejection);
    const error = await core.hosts.paired().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RsCoreError);
    expect(error).toMatchObject({
      kind: 'http',
      message: 'm',
      httpStatus: 403,
      hubCode: 'forbidden',
    });
  });

  it('bindings.events', () => {
    const { t, core } = setup();
    const listen = vi.spyOn(t, 'listen');
    const first = vi.fn();
    const event = {
      type: 'session.state',
      sessionId: '7',
      hostId: 'h7v2m9q3k8x4tb1a',
      phase: 'streaming',
    };

    const unsubscribe = core.events.subscribe(first);
    t.emit(1, event);
    expect(first).toHaveBeenCalledTimes(1);
    expect(first).toHaveBeenCalledWith(event);

    unsubscribe();
    t.emit(2, { ...event, phase: 'ended' });
    expect(first).toHaveBeenCalledTimes(1);

    const second = vi.fn();
    core.events.subscribe(second);
    t.emit(3, { ...event, phase: 'reconnecting' });
    expect(second).toHaveBeenCalledWith({ ...event, phase: 'reconnecting' });
    expect(listen).toHaveBeenCalledTimes(1);
    expect(listen).toHaveBeenCalledWith('rscore://event', expect.any(Function));
    expect(t.calls).toEqual([]);
  });

  it('bindings.parse_qr', () => {
    const { t, core } = setup();

    expect(core.pairing.parseQr('任意文本')).toBeNull();
    expect(core.pairing.parseQr('rsp://pair?v=1&h=h7&fp=x&c=123456&n=Mac&a=')).toBeNull();
    expect(t.calls).toEqual([]);
  });

  it('bindings.sessions_list_unsupported', () => {
    const { t, core } = setup();

    const error = caught(() => core.sessions.list());
    expect(error).toBeInstanceOf(RsCoreError);
    expect(error).toMatchObject({ kind: 'unsupported', message: 'sessions.list: use desktop' });
    expect(t.calls).toEqual([]);
  });

  it('bindings.fire_and_forget_warns', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { t, core } = setup({
      rscore: {
        sessionsClose: () => {
          throw { tag: 'Internal', inner: { message: 'session 9876543210 gone' } };
        },
      },
    });

    core.sessions.close('9876543210');

    await vi.waitFor(() => expect(warn).toHaveBeenCalledTimes(1));
    expect(warn).toHaveBeenCalledWith('rscore call failed', 'sessionsClose');
    expect(JSON.stringify(warn.mock.calls)).not.toContain('9876543210');
    expect(t.calls).toEqual([
      { cmd: 'rscore_call', args: { name: 'sessionsClose', args: ['9876543210'] } },
    ]);
  });

  it('bindings.desktop_absent_methods', async () => {
    const { t, b, core } = setup();

    await expect(core.ssh.servers()).rejects.toMatchObject({ kind: 'unsupported' });
    expect(caught(() => core.ssh.close('1'))).toMatchObject({ kind: 'unsupported' });
    await expect(core.files.list('h7v2m9q3k8x4tb1a', null)).rejects.toMatchObject({
      kind: 'unsupported',
    });
    await expect(core.tailnet.available()).resolves.toBe(false);
    expect(caught(() => core.sessions.setAudio('1', true))).toMatchObject({
      kind: 'unsupported',
    });
    expect(b.sessionsSetAudio).toBeUndefined();
    expect(b.crashRecordJs).toBeUndefined();
    expect(b.transfersList).toBeUndefined();
    expect(t.calls).toEqual([]);
  });

  it('bindings.core_init', async () => {
    const { t, core } = setup();
    const cfg = {
      dataDir: '/tmp/core',
      appVersion: '0.1.0',
      platform: 'ios' as const,
    };

    await expect(core.init(cfg)).resolves.toBeUndefined();
    expect(t.calls).toEqual([
      { cmd: 'rscore_call', args: { name: 'coreInit', args: [JSON.stringify(cfg)] } },
    ]);
  });
});
