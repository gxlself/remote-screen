import { describe, expect, it, vi } from 'vitest';

import { RsCoreError } from '@app/native/rscore/RsCoreApi';

import { desktopApi, fileUri, type DesktopSettings, type OpenSessionArgs } from './desktopApi';
import { FAKE_INFO, FAKE_SETTINGS, fakeTransport } from './fake';

describe('desktopApi', () => {
  it('desktopApi.open', async () => {
    const t = fakeTransport();
    const req: OpenSessionArgs = {
      hostId: 'h7v2m9q3k8x4tb1a',
      hostName: 'Studio Mac mini',
      hostOs: 'macos',
      control: true,
      displayId: 2,
      quality: 'clear',
    };

    await expect(desktopApi(t).openSession(req)).resolves.toBe('1');
    expect(t.calls).toEqual([{ cmd: 'desktop_open_session', args: { req } }]);
  });

  it('desktopApi.commands', async () => {
    const t = fakeTransport();
    const api = desktopApi(t);
    const next: DesktopSettings = { ...FAKE_SETTINGS, defaultQuality: 'eco' };

    await expect(api.info()).resolves.toEqual(FAKE_INFO);
    await expect(api.settings()).resolves.toEqual(FAKE_SETTINGS);
    await expect(api.setSettings(next)).resolves.toBeUndefined();
    await expect(api.settings()).resolves.toEqual(next);
    expect(t.calls).toEqual([
      { cmd: 'desktop_info', args: undefined },
      { cmd: 'desktop_settings_get', args: undefined },
      { cmd: 'desktop_settings_set', args: { settings: next } },
      { cmd: 'desktop_settings_get', args: undefined },
    ]);
  });

  it('desktopApi.errors_are_rs_core_errors', async () => {
    const t = fakeTransport();
    vi.spyOn(t, 'invoke').mockRejectedValue({
      tag: 'Network',
      inner: { message: 'host offline' },
    });

    const error = await desktopApi(t)
      .openSession({ hostId: 'h', hostName: 'h', hostOs: 'unknown', control: false })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RsCoreError);
    expect(error).toMatchObject({ kind: 'network', message: 'host offline' });
  });

  it('desktopApi.on_session', async () => {
    const t = fakeTransport();
    const cb = vi.fn();

    const stop = await desktopApi(t).onSession(cb);
    t.emitSession({ type: 'focus', sessionId: '3', focused: true });
    stop();
    t.emitSession({ type: 'ended', sessionId: '3', hostId: 'h', byUser: true });

    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith({ type: 'focus', sessionId: '3', focused: true });
  });

  it.each([
    ['/Users/a b/配对.rspair', 'file:///Users/a%20b/%E9%85%8D%E5%AF%B9.rspair'],
    ['C:\\Users\\a\\x.rspair', 'file:///C:/Users/a/x.rspair'],
    [null, null],
  ])('desktopApi.pick_file_uri %s', async (picked, uri) => {
    const t = fakeTransport({ pickFile: picked });
    const pickFile = vi.spyOn(t, 'pickFile');

    await expect(desktopApi(t).pickPairFile()).resolves.toBe(uri);
    expect(pickFile).toHaveBeenCalledWith('选择配对文件', ['rspair']);
  });

  it('desktopApi.file_uri_round_trips', () => {
    const posix = '/Users/demo/Downloads/Studio #2 ?100%.rspair';
    const uri = fileUri(posix);
    expect(uri).toBe('file:///Users/demo/Downloads/Studio%20%232%20%3F100%25.rspair');
    expect(decodeURIComponent(new URL(uri).pathname)).toBe(posix);

    expect(fileUri('D:/共享/x.rspair')).toBe('file:///D:/%E5%85%B1%E4%BA%AB/x.rspair');
    expect(fileUri('\\\\nas\\share\\x.rspair')).toBe('file://nas/share/x.rspair');
  });
});
