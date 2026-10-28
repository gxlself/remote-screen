import { act, fireEvent } from '@testing-library/react-native';
import { Dimensions, StyleSheet } from 'react-native';
import { useSessions } from '../../features/session/store';
import { liveSession } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import DisplaySheet from './DisplaySheet';
import type { SessionCtx } from './slots';

const displays = [
  {
    id: 1,
    name: 'Studio Display',
    widthPx: 2560,
    heightPx: 1600,
    primary: true,
    refreshHz: 60,
  },
  {
    id: 2,
    name: '竖屏显示器',
    widthPx: 2160,
    heightPx: 3840,
    primary: false,
    refreshHz: 60,
  },
];

function Harness({ ctx, onClose }: { ctx: SessionCtx; onClose: () => void }) {
  const rec = useSessions(state => state[liveSession.id]);
  return <DisplaySheet ctx={{ ...ctx, rec }} onClose={onClose} />;
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

async function mount(options: { currentId?: number } = {}) {
  const core = createFakeCore({ sessions: [liveSession] });
  const onClose = jest.fn();
  const view = await renderWithProviders(
    <Harness ctx={baseCtx()} onClose={onClose} />,
    { core },
  );
  await act(async () => {
    core.emit({
      type: 'session.displays',
      sessionId: liveSession.id,
      displays,
      currentId: options.currentId ?? 1,
    });
  });
  return { ...view, core, onClose };
}

describe('DisplaySheet', () => {
  it('shows both displays, their metadata and the current one selected', async () => {
    const view = await mount();

    expect(view.getByText('选择要看的显示器')).toBeOnTheScreen();
    expect(
      view.getByText('Studio Mac mini · 按 Mac 上「显示器排列」的位置摆放'),
    ).toBeOnTheScreen();
    expect(view.getByText('显示器 1 · 主显示器')).toBeOnTheScreen();
    expect(view.getByText('2560×1600 · 60 Hz')).toBeOnTheScreen();
    expect(view.getByText('显示器 2 · 竖放')).toBeOnTheScreen();
    expect(view.getByText('正在看')).toBeOnTheScreen();
    expect(view.getByTestId('display-thumb-1')).toHaveStyle({
      width: 272,
      height: 170,
      borderWidth: 2.5,
    });
    expect(view.getByTestId('display-thumb-2')).toHaveStyle({
      width: 108,
      height: 192,
      borderWidth: 1,
    });

    const radios = view.getAllByRole('radio');
    expect(radios).toHaveLength(2);
    expect(radios[0]).toBeSelected();
    expect(radios[1].props.accessibilityState.selected).toBe(false);
    expect(view.getByLabelText('关闭')).toHaveProp(
      'accessibilityRole',
      'button',
    );
  });

  it('switches to the second display with one tap and closes', async () => {
    const view = await mount();
    await fireEvent.press(view.getByRole('radio', { name: /显示器 2 · 竖放/ }));
    expect(view.core.calls).toContainEqual({
      method: 'sessions.selectDisplay',
      args: [liveSession.id, 2],
    });
    expect(view.onClose).toHaveBeenCalledTimes(1);
    expect(view.queryByText(/看显示器/)).toBeNull();
  });

  it('closes without reselecting when the current display is tapped', async () => {
    const view = await mount();
    await fireEvent.press(view.getByRole('radio', { name: /显示器 1/ }));
    expect(
      view.core.calls.some(call => call.method === 'sessions.selectDisplay'),
    ).toBe(false);
    expect(view.onClose).toHaveBeenCalledTimes(1);
  });

  it('shrinks thumbnails to fit three displays on a narrow window', async () => {
    const dimensions = jest
      .spyOn(Dimensions, 'get')
      .mockReturnValue({ width: 600, height: 400, scale: 2, fontScale: 1 });
    const core = createFakeCore({ sessions: [liveSession] });
    const view = await renderWithProviders(
      <Harness ctx={baseCtx()} onClose={jest.fn()} />,
      { core },
    );
    await act(async () => {
      core.emit({
        type: 'session.displays',
        sessionId: liveSession.id,
        displays: [
          { ...displays[0], id: 1 },
          { ...displays[0], id: 2, primary: false },
          { ...displays[0], id: 3, primary: false },
        ],
        currentId: 1,
      });
    });
    const widths = [1, 2, 3].map(
      id =>
        StyleSheet.flatten(view.getByTestId(`display-thumb-${id}`).props.style)
          .width as number,
    );
    expect(
      widths.reduce((sum, width) => sum + width, 0) + 14 * 2,
    ).toBeLessThanOrEqual(600 - 48);
    expect(view.getByTestId('display-thumb-3')).toHaveTextContent('3');
    dimensions.mockRestore();
  });

  it('describes the Windows display settings for a Windows host', async () => {
    const core = createFakeCore({ sessions: [liveSession] });
    const view = await renderWithProviders(
      <Harness ctx={baseCtx({ os: 'windows' })} onClose={jest.fn()} />,
      { core },
    );
    expect(
      view.getByText(/按电脑上「显示设置」里的位置摆放/),
    ).toBeOnTheScreen();
    expect(view.queryByText(/Mac 上/)).toBeNull();
  });
});
