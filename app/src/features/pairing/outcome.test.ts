import { createSettingsStore } from '../settings/store';
import { MemoryKv } from '../storage/kv';
import { describeFailure, formatRetryAfter } from './outcome';

const HOST_NAME = 'Studio Mac mini';

describe('pairing failure outcomes', () => {
  it('describes a wrong code with attempts remaining', () => {
    expect(
      describeFailure(
        { status: 'badCode', attemptsLeft: 3 },
        HOST_NAME,
        'macos',
      ),
    ).toEqual({
      icon: 'alert',
      tone: 'warn',
      title: '配对码不对',
      message: '还能再试 3 次',
      primary: { label: '重新输入配对码', action: 'reenterCode' },
      secondary: { label: '关闭', action: 'close' },
    });
  });

  it('asks for a new code after a typed code is exhausted', () => {
    expect(
      describeFailure(
        { status: 'badCode', attemptsLeft: 0 },
        HOST_NAME,
        'macos',
      ),
    ).toMatchObject({
      title: '配对码不对',
      message: '这个码已失效，在 Mac 上重新显示配对码',
      primary: { label: '重新输入配对码', action: 'reenterCode' },
      secondary: { label: '关闭', action: 'close' },
    });
  });

  it('offers rescan after a scanned code is exhausted', () => {
    expect(
      describeFailure(
        { status: 'badCode', attemptsLeft: 0 },
        HOST_NAME,
        'macos',
        'qr',
      ).primary,
    ).toEqual({ label: '重新扫码', action: 'rescan' });
  });

  it('describes an expired code', () => {
    expect(describeFailure({ status: 'expired' }, HOST_NAME, 'macos')).toEqual({
      icon: 'clock',
      tone: 'warn',
      title: '配对码已过期',
      message: '配对码 5 分钟内有效，只能用一次。在 Mac 上重新显示配对码',
      primary: { label: '重新输入配对码', action: 'reenterCode' },
      secondary: { label: '关闭', action: 'close' },
    });
  });

  it('sends an expired QR back to the camera with QR wording', () => {
    expect(
      describeFailure({ status: 'expired' }, HOST_NAME, 'macos', 'qr'),
    ).toMatchObject({
      message: '二维码 5 分钟内有效，只能用一次。在 Mac 上重新显示二维码，再扫一次',
      primary: { label: '重新扫码', action: 'rescan' },
    });
  });

  it('describes a rejection on the host', () => {
    expect(describeFailure({ status: 'denied' }, HOST_NAME, 'macos')).toEqual({
      icon: 'blocked',
      tone: 'warn',
      title: 'Mac 上拒绝了',
      message: 'Studio Mac mini 前的人点了「拒绝」',
      primary: { label: '重新输入配对码', action: 'reenterCode' },
      secondary: { label: '关闭', action: 'close' },
    });
  });

  it('describes the local approval timeout', () => {
    expect(describeFailure({ status: 'timeout' }, HOST_NAME, 'macos')).toEqual({
      icon: 'clock',
      tone: 'warn',
      title: '没人点「允许」',
      message: 'Mac 已自动拒绝，重新输入配对码就行',
      primary: { label: '重新输入配对码', action: 'reenterCode' },
      secondary: { label: '关闭', action: 'close' },
    });
    expect(
      describeFailure({ status: 'timeout' }, HOST_NAME, 'macos', 'qr'),
    ).toMatchObject({
      message: 'Mac 已自动拒绝，重新扫码就行',
      primary: { label: '重新扫码', action: 'rescan' },
    });
  });

  it('describes a temporarily locked host', () => {
    expect(describeFailure({ status: 'locked' }, HOST_NAME, 'macos')).toEqual({
      icon: 'lock',
      tone: 'bad',
      title: '尝试太多次',
      message: '这台电脑暂时不接受新的配对，过几分钟再试',
      primary: { label: '关闭', action: 'close' },
    });
  });

  it('keeps the default locked wording without a retry delay', () => {
    expect(describeFailure({ status: 'locked' }, HOST_NAME, 'macos')).toEqual(
      describeFailure({ status: 'locked' }, HOST_NAME, 'macos', 'code'),
    );
  });

  it('shows how long until the host unlocks', () => {
    expect(
      describeFailure(
        { status: 'locked', retryAfterS: 873 },
        HOST_NAME,
        'macos',
      ).message,
    ).toBe('请 15 分钟后再试');
  });

  it('describes pairing disabled on the current path', () => {
    createSettingsStore(new MemoryKv({ 'settings.tailnet.enabled': 'true' }));
    expect(
      describeFailure({ status: 'disabledOnPath' }, HOST_NAME, 'macos'),
    ).toEqual({
      icon: 'wifi',
      tone: 'warn',
      title: '这条路不允许配对',
      message: '这台电脑没打开「允许新设备经内网配对」，连上 Tailscale 再试',
      primary: { label: '关闭', action: 'close' },
    });
  });

  it('does not suggest Tailscale for a path without pairing while it is off', () => {
    createSettingsStore(new MemoryKv());
    expect(
      describeFailure({ status: 'disabledOnPath' }, HOST_NAME, 'macos'),
    ).toEqual({
      icon: 'wifi',
      tone: 'warn',
      title: '这条路不允许配对',
      message:
        '这台电脑没打开「允许新设备经内网配对」，在电脑的 Host 里打开后再试',
      primary: { label: '关闭', action: 'close' },
    });
  });

  it('describes a man in the middle as a hard failure', () => {
    expect(describeFailure({ status: 'mitm' }, HOST_NAME, 'macos')).toEqual({
      icon: 'shield',
      tone: 'bad',
      title: '身份校验失败',
      message: '对方的证书和二维码里的不一致，可能连到了另一台电脑。已停止配对',
      primary: { label: '关闭', action: 'close' },
    });
  });

  it('shows a Chinese error message and offers retry', () => {
    expect(
      describeFailure(
        { status: 'error', message: '网络连接中断' },
        HOST_NAME,
        'macos',
      ),
    ).toEqual({
      icon: 'warning',
      tone: 'warn',
      title: '配对没有完成',
      message: '网络连接中断',
      primary: { label: '重试', action: 'retry' },
      secondary: { label: '关闭', action: 'close' },
    });
  });

  it('replaces an English diagnostic message with a user-facing fallback', () => {
    expect(
      describeFailure(
        { status: 'error', message: 'connection reset by peer' },
        HOST_NAME,
        'macos',
      ).message,
    ).toBe('出了点问题，稍后再试');
  });

  it('uses the Windows host word when the host system is known', () => {
    const model = describeFailure({ status: 'denied' }, 'DESKTOP-1', 'windows');

    expect(model.title).toContain('电脑');
    expect(model.message).toBe('DESKTOP-1 前的人点了「拒绝」');
  });

  it('calls a host of unknown system a computer, never a Mac', () => {
    expect(
      describeFailure({ status: 'timeout' }, HOST_NAME, null).message,
    ).toBe('电脑已自动拒绝，重新输入配对码就行');
    expect(
      describeFailure({ status: 'denied' }, HOST_NAME, null, 'qr').title,
    ).toBe('电脑上拒绝了');
  });

  it.each([
    [0, '0 秒'],
    [59, '59 秒'],
    [60, '1 分钟'],
    [65, '2 分钟'],
    [873, '15 分钟'],
  ])('formats %s retry seconds as %s', (seconds, expected) => {
    expect(formatRetryAfter(seconds)).toBe(expected);
  });

  it.each([
    ['qr', '改用配对码配对'],
    ['code', '改用配对码配对'],
    ['file', '改用配对码配对'],
  ] as const)('describes a disabled %s pairing method', (method, message) => {
    expect(
      describeFailure({ status: 'methodDisabled' }, HOST_NAME, 'macos', method),
    ).toMatchObject({
      title: '这台电脑没打开这种配对方式',
      message,
      primary: { label: '换一种方式', action: 'otherMethod' },
    });
  });

  it('describes an expired pairing file', () => {
    expect(
      describeFailure({ status: 'expired' }, HOST_NAME, 'macos', 'file'),
    ).toMatchObject({
      title: '配对文件已失效',
      message: '文件已用过、被撤销或已过期。在电脑上重新导出一份',
      primary: { label: '导入别的文件', action: 'reimportFile' },
    });
  });

  it('describes a rejected pairing file with attempts remaining', () => {
    expect(
      describeFailure(
        { status: 'badCode', attemptsLeft: 2 },
        HOST_NAME,
        'macos',
        'file',
      ),
    ).toMatchObject({
      title: '配对文件校验失败',
      message: '文件可能被改过，还能再试 2 次',
      primary: { label: '导入别的文件', action: 'reimportFile' },
    });
  });

  it('asks the user to import the pairing file again when it is unloaded', () => {
    expect(
      describeFailure(
        { status: 'error', message: 'pairing file not loaded' },
        HOST_NAME,
        'macos',
        'file',
      ),
    ).toMatchObject({
      title: '请重新导入配对文件',
      primary: { label: '导入别的文件', action: 'reimportFile' },
    });
  });

  it('describes an old host that does not recognize pairing files', () => {
    expect(
      describeFailure(
        { status: 'error', code: 170, message: 'host too old' },
        HOST_NAME,
        'macos',
        'file',
      ),
    ).toMatchObject({
      title: '这台电脑上的 Host 版本太旧，不认识配对文件',
      primary: { label: '换一种方式', action: 'otherMethod' },
    });
  });

  it('does not build a failure page for cancelled pairing', () => {
    expect(() =>
      describeFailure({ status: 'cancelled' }, HOST_NAME, 'macos'),
    ).toThrow('Cancelled pairing does not have a failure page');
  });

  it('does not build a failure page for a successful pairing', () => {
    expect(() =>
      describeFailure(
        { status: 'ok', hostId: 'host-1', hostName: HOST_NAME },
        HOST_NAME,
        'macos',
      ),
    ).toThrow('Successful pairing does not have a failure page');
  });
});
