import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { fpDisplay } from '../../features/format/fingerprint';
import {
  clearPairFiles,
  rememberPairFile,
} from '../../features/pairing/pairFileCache';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  pairFileSummary,
  pairQrStudio,
  pairTargetStudio,
} from '../../native/rscore/fixtures';
import type { CoreEvent, PairOutcome } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import PairWaitingScreen from '.';

const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';

function props(params: RootScreenProps<'PairWaiting'>['route']['params']) {
  return {
    navigation: mockNavigation<'PairWaiting'>(),
    route: mockRoute('PairWaiting', params),
  } as RootScreenProps<'PairWaiting'>;
}

async function renderPairWaiting(
  params: RootScreenProps<'PairWaiting'>['route']['params'] = {
    qrText: pairQrStudio,
  },
) {
  const core = createFakeCore();
  const screenProps = props(params);
  // The tailnet gives the paired Hosts their node names (外网访问 on).
  const view = await renderWithProviders(
    <PairWaitingScreen {...screenProps} />,
    {
      core,
      settings: { 'tailnet.enabled': true },
      localDirectory: { peers: [] },
    },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, ...screenProps, core };
}

async function emit(core: ReturnType<typeof createFakeCore>, event: CoreEvent) {
  await act(async () => {
    core.emit(event);
  });
}

async function resolve(
  core: ReturnType<typeof createFakeCore>,
  outcome: PairOutcome,
) {
  await act(async () => {
    core.resolvePairing(outcome);
    await Promise.resolve();
  });
}

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
  clearPairFiles();
});

describe('PairWaiting', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  it('starts pairing from the QR target and shows the host title', async () => {
    const { core } = await renderPairWaiting();
    const starts = core.calls.filter(call => call.method === 'pairing.start');
    expect(starts).toHaveLength(1);
    expect(starts[0].args[0]).toMatchObject({
      hostId: STUDIO_HOST_ID,
      code: '482913',
    });
    expect(core.calls).toContainEqual({
      method: 'pairing.parseQr',
      args: [pairQrStudio],
    });
    expect(screen.getByText('正在和 Studio Mac mini 配对')).toBeOnTheScreen();
    expect(
      screen.getByText('studio-mac · 100.64.0.7 · Host 0.3.1'),
    ).toBeOnTheScreen();
  });

  it('advances three steps, shows path/fingerprint and ticks the countdown', async () => {
    const { core } = await renderPairWaiting();

    await emit(core, { type: 'pairing.progress', step: 'dialing' });
    expect(screen.getByTestId('step-connect')).toHaveProp(
      'accessibilityState',
      { busy: true },
    );

    await emit(core, {
      type: 'pairing.progress',
      step: 'verifying',
      fp: pairTargetStudio.fp,
      path: 'tailnetDirect',
      latencyMs: 6,
    });
    expect(screen.getByLabelText('连上电脑，已完成')).toBeOnTheScreen();
    expect(screen.getByLabelText('直连 6 毫秒')).toBeOnTheScreen();
    expect(screen.getByLabelText('证书指纹一致，进行中')).toBeOnTheScreen();
    const displayFp = fpDisplay(pairTargetStudio.fp);
    const fingerprintGroups = displayFp.split(' ');
    expect(fingerprintGroups).toHaveLength(4);
    fingerprintGroups.forEach(group => expect(group).toHaveLength(4));
    expect(screen.getByText(displayFp)).toBeOnTheScreen();
    expect(
      screen.getByLabelText(`指纹 ${fingerprintGroups.join('，')}`),
    ).toBeOnTheScreen();

    await emit(core, { type: 'pairing.progress', step: 'waitingLocal' });
    expect(
      screen.getByLabelText('等待你在 Mac 上点「允许」，进行中'),
    ).toBeOnTheScreen();
    expect(screen.getByLabelText('倒计时 30 秒')).toBeOnTheScreen();

    await act(async () => {
      jest.advanceTimersByTime(2000);
    });
    expect(screen.getByLabelText('倒计时 28 秒')).toBeOnTheScreen();
  });

  it('navigates to PairDone after a successful pairing', async () => {
    const { core, navigation } = await renderPairWaiting();
    await resolve(core, {
      status: 'ok',
      hostId: STUDIO_HOST_ID,
      hostName: 'Studio Mac mini',
      permission: 'unattended',
      path: 'tailnetDirect',
      latencyMs: 6,
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(navigation.replace).toHaveBeenCalledWith('PairDone', {
      hostId: STUDIO_HOST_ID,
      hostName: 'Studio Mac mini',
      permission: 'unattended',
      path: 'tailnetDirect',
      latencyMs: 6,
    });
  });

  it('shows bad code attempts and re-enters the code flow', async () => {
    const { core, navigation } = await renderPairWaiting();
    await resolve(core, { status: 'badCode', attemptsLeft: 3 });
    expect(screen.getByText('配对码不对')).toBeOnTheScreen();
    expect(screen.getByText('还能再试 3 次')).toBeOnTheScreen();
    expect(screen.getByLabelText('证书指纹一致，失败')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '重新输入配对码' }));
    // The code page opens for this computer with its address filled in.
    expect(navigation.replace).toHaveBeenCalledWith(
      'PairAdd',
      expect.objectContaining({ tab: 'code', hostId: STUDIO_HOST_ID }),
    );
  });

  it('shows denied, mitm and retry failures', async () => {
    const { core, navigation } = await renderPairWaiting();
    await resolve(core, { status: 'denied' });
    expect(screen.getByText('Mac 上拒绝了')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '重新扫码' }));
    expect(navigation.replace).toHaveBeenCalledWith('PairAdd', {
      tab: 'scan',
    });
  });

  it('shows a fingerprint mismatch failure with a single close button', async () => {
    const { core } = await renderPairWaiting();
    await resolve(core, { status: 'mitm' });
    expect(screen.getByText('身份校验失败')).toBeOnTheScreen();
    expect(screen.getAllByRole('button', { name: '关闭' })).toHaveLength(1);
    expect(screen.queryByTestId('pair-failure-close')).toBeNull();
  });

  it('shows a timeout failure', async () => {
    const { core } = await renderPairWaiting();
    await resolve(core, { status: 'timeout' });
    expect(screen.getByText('没人点「允许」')).toBeOnTheScreen();
    expect(screen.getByText('Mac 已自动拒绝，重新扫码就行')).toBeOnTheScreen();
  });

  it('retries after an error by starting pairing again', async () => {
    const { core } = await renderPairWaiting();
    await resolve(core, { status: 'error', message: 'network down' });
    expect(screen.getByText('配对没有完成')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '重试' }));
    expect(
      core.calls.filter(call => call.method === 'pairing.start'),
    ).toHaveLength(2);
  });

  it('cancels explicitly and on unmount', async () => {
    const first = await renderPairWaiting();
    await fireEvent.press(screen.getByLabelText('取消配对'));
    expect(first.core.calls).toContainEqual({
      method: 'pairing.cancel',
      args: [],
    });
    expect(first.navigation.goBack).toHaveBeenCalled();
    await first.unmount();

    const second = await renderPairWaiting();
    await second.unmount();
    expect(second.core.calls).toContainEqual({
      method: 'pairing.cancel',
      args: [],
    });
  });

  it('rejects an invalid QR without starting pairing', async () => {
    const { core } = await renderPairWaiting({ qrText: 'hello' });
    expect(screen.getByText('二维码无效')).toBeOnTheScreen();
    expect(core.calls.some(call => call.method === 'pairing.start')).toBe(
      false,
    );
  });

  it('uses 电脑 wording for a Windows host', async () => {
    // The wording follows the paired Host's OS, not the name in the QR.
    await renderPairWaiting({
      qrText: pairQrStudio.replace(STUDIO_HOST_ID, OFFICE_HOST_ID),
    });
    expect(
      screen.getByText('等待你在电脑上点「允许」'),
    ).toBeOnTheScreen();
  });

  it('starts a cached pairing file and shows only two steps', async () => {
    rememberPairFile(pairFileSummary);
    const { core } = await renderPairWaiting({
      fileId: pairFileSummary.fileId,
    });

    const starts = core.calls.filter(call => call.method === 'pairing.start');
    expect(starts).toHaveLength(1);
    expect(starts[0].args[0]).toEqual({ fileId: pairFileSummary.fileId });
    expect(
      screen.getByText(`正在和 ${pairFileSummary.hostName} 配对`),
    ).toBeOnTheScreen();
    expect(screen.getByTestId('step-connect')).toBeOnTheScreen();
    expect(screen.getByTestId('step-verify')).toBeOnTheScreen();
    expect(screen.queryByTestId('step-approve')).toBeNull();
  });

  it('replaces PairDone after a successful file pairing', async () => {
    rememberPairFile(pairFileSummary);
    const { core, navigation } = await renderPairWaiting({
      fileId: pairFileSummary.fileId,
    });
    await resolve(core, {
      status: 'ok',
      hostId: pairFileSummary.hostId,
      hostName: pairFileSummary.hostName,
      permission: 'control',
    });
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(navigation.replace).toHaveBeenCalledWith('PairDone', {
      hostId: pairFileSummary.hostId,
      hostName: pairFileSummary.hostName,
      permission: 'control',
    });
  });

  it('reimports after an expired pairing file', async () => {
    rememberPairFile(pairFileSummary);
    const { core, navigation } = await renderPairWaiting({
      fileId: pairFileSummary.fileId,
    });
    await resolve(core, { status: 'expired' });

    expect(screen.getByText('配对文件已失效')).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '导入别的文件' }));
    expect(navigation.replace).toHaveBeenCalledWith('PairFile');
  });

  it('switches method when the file host is too old', async () => {
    rememberPairFile(pairFileSummary);
    const { core, navigation } = await renderPairWaiting({
      fileId: pairFileSummary.fileId,
    });
    await resolve(core, {
      status: 'error',
      code: 170,
      message: 'host too old',
    });

    expect(
      screen.getByText('这台电脑上的 Host 版本太旧，不认识配对文件'),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '换一种方式' }));
    expect(navigation.replace).toHaveBeenCalledWith(
      'PairAdd',
      expect.objectContaining({ tab: 'code', hostId: pairFileSummary.hostId }),
    );
  });

  it('starts with an unknown host when the file summary was evicted', async () => {
    const { core, navigation } = await renderPairWaiting({
      fileId: 'missing-file-id',
    });

    expect(
      core.calls.filter(call => call.method === 'pairing.start'),
    ).toHaveLength(1);
    expect(screen.getByText('正在和 未知电脑 配对')).toBeOnTheScreen();

    await resolve(core, { status: 'methodDisabled' });
    await fireEvent.press(screen.getByRole('button', { name: '换一种方式' }));
    expect(navigation.replace).toHaveBeenCalledWith('PairAdd', undefined);
  });

  describe('by address', () => {
    const byAddress = { addr: 'home-mac', port: 7400, code: '482913' };

    it('starts pairing with the address and shows the probing step', async () => {
      const { core } = await renderPairWaiting(byAddress);
      const starts = core.calls.filter(call => call.method === 'pairing.start');
      expect(starts).toHaveLength(1);
      expect(starts[0].args[0]).toEqual({
        addr: 'home-mac',
        port: 7400,
        code: '482913',
      });
      expect(screen.getByText('正在和 这台电脑 配对')).toBeOnTheScreen();
      expect(screen.getByText('home-mac')).toBeOnTheScreen();
      expect(
        screen.getByLabelText('连上电脑，正在连接这台电脑…，进行中'),
      ).toBeOnTheScreen();

      await emit(core, {
        type: 'pairing.progress',
        step: 'verifying',
        fp: pairTargetStudio.fp,
      });
      expect(
        screen.getByLabelText('连上电脑，已完成'),
      ).toBeOnTheScreen();
      expect(screen.queryByText('正在连接这台电脑…')).not.toBeOnTheScreen();
    });

    it('passes no port when the route has none', async () => {
      const { core } = await renderPairWaiting({
        addr: '100.64.0.3',
        code: '482913',
      });
      const start = core.calls.find(call => call.method === 'pairing.start');
      expect(start?.args[0]).toEqual({ addr: '100.64.0.3', code: '482913' });
    });

    it('explains an address without Remote Host and returns to the form', async () => {
      const { core, navigation } = await renderPairWaiting(byAddress);
      await resolve(core, { status: 'error', message: 'not a remote host' });
      expect(screen.getByText('配对没有完成')).toBeOnTheScreen();
      expect(
        screen.getByText('这个地址上没有 Host。'),
      ).toBeOnTheScreen();
      await fireEvent.press(screen.getByRole('button', { name: '重新输入配对码' }));
      expect(navigation.replace).toHaveBeenCalledWith('PairAdd', {
        tab: 'code',
        addr: 'home-mac',
        port: 7400,
      });
    });

    it('recognises a thrown not-a-host error too', async () => {
      const core = createFakeCore();
      jest
        .spyOn(core.pairing, 'start')
        .mockRejectedValue(new RsCoreError('internal', 'not a remote host'));
      await renderWithProviders(<PairWaitingScreen {...props(byAddress)} />, {
        core,
        settings: { 'tailnet.enabled': true },
        localDirectory: { peers: [] },
      });
      await act(async () => {
        await jest.advanceTimersByTimeAsync(1);
      });
      expect(
        screen.getByText('这个地址上没有 Host。'),
      ).toBeOnTheScreen();
    });

    it('keeps other errors as they are', async () => {
      const { core } = await renderPairWaiting(byAddress);
      await resolve(core, { status: 'error', message: 'network down' });
      expect(screen.getByText('配对没有完成')).toBeOnTheScreen();
      expect(screen.getByText('出了点问题，稍后再试')).toBeOnTheScreen();
      expect(
        screen.queryByText('这个地址上没有 Host。'),
      ).not.toBeOnTheScreen();
      await fireEvent.press(screen.getByRole('button', { name: '重试' }));
      expect(
        core.calls.filter(call => call.method === 'pairing.start'),
      ).toHaveLength(2);
    });

    it('re-enters a bad code at the same address', async () => {
      const { core, navigation } = await renderPairWaiting(byAddress);
      await resolve(core, { status: 'badCode', attemptsLeft: 3 });
      expect(screen.getByText('配对码不对')).toBeOnTheScreen();
      await fireEvent.press(screen.getByRole('button', { name: '重新输入配对码' }));
      expect(navigation.replace).toHaveBeenCalledWith('PairAdd', {
        tab: 'code',
        addr: 'home-mac',
        port: 7400,
      });
    });

    it('keeps the address when an expired or unanswered code is typed again', async () => {
      const { core, navigation } = await renderPairWaiting(byAddress);
      await resolve(core, { status: 'timeout' });
      expect(screen.getByText('没人点「允许」')).toBeOnTheScreen();
      expect(
        screen.queryByRole('button', { name: '重新扫码' }),
      ).not.toBeOnTheScreen();
      await fireEvent.press(
        screen.getByRole('button', { name: '重新输入配对码' }),
      );
      expect(navigation.replace).toHaveBeenCalledWith('PairAdd', {
        tab: 'code',
        addr: 'home-mac',
        port: 7400,
      });
    });

        it('leaves the not-a-host wording to address pairing', async () => {
      const { core } = await renderPairWaiting();
      await resolve(core, { status: 'error', message: 'not a remote host' });
      expect(screen.getByText('出了点问题，稍后再试')).toBeOnTheScreen();
      expect(
        screen.queryByText('这个地址上没有 Host。'),
      ).not.toBeOnTheScreen();
    });
  });
});
