import { act, fireEvent } from '@testing-library/react-native';
import { useSessions } from '../../features/session/store';
import { liveSession } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { CoreEventOf } from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import QualitySheet from './QualitySheet';
import type { SessionCtx } from './slots';

const stats: CoreEventOf<'session.stats'> = {
  type: 'session.stats',
  sessionId: liveSession.id,
  rttMs: 6,
  fps: 60,
  bitrateKbps: 18_400,
  decodeMs: 3,
  lossPct: 0.1,
  width: 2560,
  height: 1600,
  codec: 'hevc',
  path: 'tailnetDirect',
  transport: 'quic',
};

const displays = [
  {
    id: 1,
    name: '显示器 1',
    widthPx: 2560,
    heightPx: 1600,
    primary: true,
    refreshHz: 60,
  },
];

function useRecord(sessionId: string | null) {
  return useSessions(state =>
    sessionId === null ? undefined : state[sessionId],
  );
}

function Harness({
  sessionId,
  ctx,
  onClose,
}: {
  sessionId: string | null;
  ctx: SessionCtx;
  onClose: () => void;
}) {
  const rec = useRecord(sessionId);
  return <QualitySheet ctx={{ ...ctx, rec }} onClose={onClose} />;
}

function baseCtx(overrides: Partial<SessionCtx> = {}): SessionCtx {
  return {
    hostId: liveSession.hostId,
    hostName: 'Studio Mac mini',
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: liveSession.id,
    rec: undefined,
    view: {
      overlay: 'none',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: false,
      shared: null,
      localUse: false,
      relay: false,
      tcp: false,
      idle: false,
      toolbar: {
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '自动',
        qualityA11y: '画质：自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: 6,
      fps: 60,
      tcp: false,
      alertText: null,
      a11y: '连接质量：直连',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: 0,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect: jest.fn(),
    cancelConnect: jest.fn(),
    openTerminal: jest.fn(),
    showToast: jest.fn(),
    ...overrides,
  };
}

async function emit(core: ReturnType<typeof createFakeCore>) {
  await act(async () => {
    core.emit({
      type: 'session.state',
      sessionId: liveSession.id,
      hostId: liveSession.hostId,
      phase: 'streaming',
      path: 'tailnetDirect',
      transport: 'quic',
      dial: [
        {
          path: 'tailnetDirect',
          addr: '192.168.31.20:41641',
          state: 'ok',
        },
      ],
    });
    core.emit({
      type: 'session.displays',
      sessionId: liveSession.id,
      displays,
      currentId: 1,
    });
    core.emit(stats);
  });
}

describe('QualitySheet', () => {
  it('renders the title, display subtitle, quality choices and stats', async () => {
    const core = createFakeCore({ sessions: [liveSession] });
    const onClose = jest.fn();
    const view = await renderWithProviders(
      <Harness ctx={baseCtx()} onClose={onClose} sessionId={liveSession.id} />,
      { core },
    );
    await emit(core);

    expect(view.getByText('连接与画质')).toBeOnTheScreen();
    expect(view.getByText('Studio Mac mini · 显示器 1')).toBeOnTheScreen();
    for (const label of ['自动', '流畅', '清晰', '省流量']) {
      expect(view.getByText(label)).toBeOnTheScreen();
    }
    expect(view.queryByText('原画')).toBeNull();
    expect(view.queryByText('远端声音')).toBeNull();
    expect(view.queryByText('截图')).toBeNull();
    expect(view.getByText('6 ms')).toBeOnTheScreen();
    expect(view.getByText('HEVC')).toBeOnTheScreen();
    expect(view.getByText('直连 192.168.31.20:41641')).toBeOnTheScreen();
    expect(view.getByTestId('quality-stats')).toHaveProp(
      'accessibilityLabel',
      '延迟 6 毫秒，帧率 60，码率 18.4 Mbps，分辨率 2560×1600，丢包 0.1%，编码 HEVC',
    );
    expect(onClose).not.toHaveBeenCalled();
  });

  it('changes quality and explains the selected level', async () => {
    const setQuality = jest.fn();
    const view = await renderWithProviders(
      <QualitySheet
        ctx={baseCtx({ quality: 'auto', setQuality })}
        onClose={jest.fn()}
      />,
    );
    await fireEvent.press(view.getByLabelText('流畅'));
    expect(setQuality).toHaveBeenCalledWith('smooth');
    await view.rerender(
      <QualitySheet
        ctx={baseCtx({ quality: 'smooth', setQuality })}
        onClose={jest.fn()}
      />,
    );
    expect(
      view.getByText('优先流畅：画面稍软一点，网络差或经中继时也不容易卡。'),
    ).toBeOnTheScreen();
  });

  it('toggles the HUD setting', async () => {
    const view = await renderWithProviders(
      <QualitySheet ctx={baseCtx()} onClose={jest.fn()} />,
    );
    expect(view.settingsStore.get('session.hud')).toBe(false);
    await fireEvent.press(view.getByLabelText('统计浮层'));
    expect(view.settingsStore.get('session.hud')).toBe(true);
  });

  it('leaves locking to the More sheet', async () => {
    // R35：锁屏只在「更多」里出现一次。
    const view = await renderWithProviders(
      <QualitySheet ctx={baseCtx()} onClose={jest.fn()} />,
    );
    expect(view.queryByLabelText('锁定屏幕')).toBeNull();
  });

  it('disconnects and closes from the labelled close button', async () => {
    const ctx = baseCtx();
    const onClose = jest.fn();
    const view = await renderWithProviders(
      <QualitySheet ctx={ctx} onClose={onClose} />,
    );
    await fireEvent.press(view.getByLabelText('断开'));
    expect(ctx.disconnect).toHaveBeenCalledTimes(1);
    await fireEvent.press(view.getByLabelText('关闭面板，回到画面'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
