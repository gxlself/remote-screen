import { renderWithProviders } from '../../test/render';
import { FakePlatform } from '../../native/platform/fakePlatform';
import ClipboardSync from './ClipboardSync';
import type { SessionRecord } from '../../features/session/store';
import type { SessionCtx } from './slots';

function ctxWith(clipboard: SessionRecord['clipboard']): SessionCtx {
  return {
    hostId: 'host-mac',
    hostName: 'Studio Mac mini',
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: 's1',
    rec: {
      id: 's1',
      hostId: 'host-mac',
      hostName: 'Studio Mac mini',
      phase: 'streaming',
      role: 'controller',
      path: 'tailnetDirect',
      transport: 'quic',
      startedAt: 1,
      displayId: 1,
      controller: null,
      viewers: [],
      inputPausedLocal: false,
      stats: null,
      displays: [],
      dial: [],
      approvalTimeoutS: null,
      derpRegion: null,
      reconnectReason: null,
      idleWarning: null,
      controlRequest: null,
      lastError: null,
      clipboard,
    },
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
        qualityA11y: '画质自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: true,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '正在测量',
    },
    isPad: false,
    reduceMotion: false,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: 0,
    phaseAtMs: {},
    firstFrameAtMs: null,
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
  };
}

const initial: SessionRecord['clipboard'] = [
  { direction: 'fromHost', text: 'old', at: 1 },
];

function ClipboardProbe({
  clipboard,
}: {
  clipboard: SessionRecord['clipboard'];
}) {
  return <ClipboardSync ctx={ctxWith(clipboard)} />;
}

async function renderSync(
  platform: FakePlatform,
  clipboard: SessionRecord['clipboard'],
  autoSync: boolean,
) {
  return renderWithProviders(<ClipboardProbe clipboard={clipboard} />, {
    platform,
    settings: { 'clipboard.autoSync': autoSync },
  });
}

it('does not copy records already present when mounted', async () => {
  const platform = new FakePlatform();
  await renderSync(platform, initial, true);
  expect(platform.copied).toEqual([]);
});

it('copies only a new fromHost head, across rerenders', async () => {
  const platform = new FakePlatform();
  const view = await renderSync(platform, initial, true);
  const next: SessionRecord['clipboard'] = [
    { direction: 'fromHost', text: 'new', at: 2 },
    ...initial,
  ];
  await view.rerender(<ClipboardProbe clipboard={next} />);
  expect(platform.copied).toEqual(['new']);

  const withToHost: SessionRecord['clipboard'] = [
    { direction: 'toHost', text: 'sent', at: 3 },
    ...next,
  ];
  await view.rerender(<ClipboardProbe clipboard={withToHost} />);
  expect(platform.copied).toEqual(['new']);
});

it('does not copy new records while auto-sync is off', async () => {
  const platform = new FakePlatform();
  const view = await renderSync(platform, initial, false);
  const next: SessionRecord['clipboard'] = [
    { direction: 'fromHost', text: 'new', at: 2 },
    ...initial,
  ];
  await view.rerender(<ClipboardProbe clipboard={next} />);
  expect(platform.copied).toEqual([]);
});
