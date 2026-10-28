import { act, fireEvent, screen } from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import { FakePlatform } from '../../native/platform/fakePlatform';
import type { SessionRecord } from '../../features/session/store';
import ClipboardSheet from './ClipboardSheet';
import type { SessionCtx } from './slots';

const SID = 's1';

function record(
  clipboard: SessionRecord['clipboard'] = [],
  overrides: Partial<SessionRecord> = {},
): SessionRecord {
  return {
    id: SID,
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
    ...overrides,
  };
}

function ctxFor(
  rec: SessionRecord = record(),
  inputBlocked = false,
): SessionCtx & {
  showToast: jest.Mock;
  disconnect: jest.Mock;
  openCoach: jest.Mock;
  openSheet: jest.Mock;
} {
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
    rec,
    view: {
      overlay: 'none',
      end: { kind: 'stay' },
      role: inputBlocked ? 'viewer' : 'controller',
      inputBlocked,
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
        keyboardEnabled: !inputBlocked,
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

function at(hour: number, minute: number): number {
  return Date.UTC(2026, 9, 21, hour, minute, 0) * 1000;
}

const fromHost128 = {
  direction: 'fromHost' as const,
  text: 'a'.repeat(128),
  at: at(10, 23),
};
const fromHostCargo = {
  direction: 'fromHost' as const,
  text: 'cargo run --release',
  at: at(10, 18),
};
const toHost = {
  direction: 'toHost' as const,
  text: 'sent to host',
  at: at(10, 5),
};

async function renderSheet(
  platform = new FakePlatform(),
  rec = record([fromHost128, fromHostCargo, toHost]),
  inputBlocked = false,
  settings: Record<string, unknown> = { 'clipboard.autoSync': true },
) {
  const ctx = ctxFor(rec, inputBlocked);
  const onClose = jest.fn();
  const view = await renderWithProviders(
    <ClipboardSheet ctx={ctx} onClose={onClose} />,
    { platform, settings },
  );
  return { ...view, ctx, onClose, platform };
}

it('renders the latest host text, synced metadata and the recent list', async () => {
  await renderSheet();
  expect(screen.getByText('电脑上复制的')).toBeOnTheScreen();
  expect(screen.getByTestId('clipboard-latest-text')).toHaveTextContent(
    'a'.repeat(128),
  );
  expect(screen.getByText('已同步 128 字 · 10:23')).toBeOnTheScreen();
  expect(screen.getByLabelText('最近 3 条剪贴板')).toBeOnTheScreen();
});

it('sends pasted text to the computer or types it', async () => {
  const rendered = await renderSheet();
  rendered.platform.clipboardText = '100.64.0.12';
  await fireEvent.press(screen.getByLabelText('粘贴到电脑'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.core.calls).toContainEqual({
    method: 'sessions.clipboardSend',
    args: [SID, '100.64.0.12'],
  });
  expect(rendered.ctx.showToast).toHaveBeenCalledWith(
    '已发到电脑，可以在电脑上粘贴',
  );

  await fireEvent.press(screen.getByLabelText('粘贴为键入'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.core.calls).toContainEqual({
    method: 'sessions.text',
    args: [SID, '100.64.0.12'],
  });
});

it('shows an empty-clipboard toast and sends nothing when readText is null', async () => {
  const rendered = await renderSheet();
  rendered.platform.clipboardText = null;
  await fireEvent.press(screen.getByLabelText('粘贴到电脑'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.ctx.showToast).toHaveBeenCalledWith('手机剪贴板是空的');
  expect(
    rendered.core.calls.filter(
      call => call.method === 'sessions.clipboardSend',
    ),
  ).toEqual([]);
});

it('tells a failed phone read apart from an empty clipboard', async () => {
  const rendered = await renderSheet();
  rendered.platform.readText = jest.fn(() => Promise.reject(new Error('x')));
  await fireEvent.press(screen.getByLabelText('粘贴到电脑'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.ctx.showToast).toHaveBeenCalledWith(
    '读不到手机剪贴板，请重试',
    { tone: 'bad' },
  );
  expect(rendered.ctx.showToast).not.toHaveBeenCalledWith('手机剪贴板是空的');
});

it('reports a failed send to the computer', async () => {
  const rendered = await renderSheet();
  rendered.platform.clipboardText = 'hello';
  rendered.core.failNext('sessions.clipboardSend', new Error('closed'));
  await fireEvent.press(screen.getByLabelText('粘贴到电脑'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.ctx.showToast).toHaveBeenCalledWith('没发到电脑，请重试', {
    tone: 'bad',
  });
  expect(rendered.ctx.showToast).not.toHaveBeenCalledWith(
    '已发到电脑，可以在电脑上粘贴',
  );
});

it('confirms typing for long text and reports what was typed', async () => {
  const rendered = await renderSheet();
  rendered.platform.clipboardText = 'short';
  await fireEvent.press(screen.getByLabelText('粘贴为键入'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.ctx.showToast).toHaveBeenCalledWith('已在电脑上键入 5 字', {
    tone: 'good',
  });

  rendered.platform.clipboardText = 'x'.repeat(240);
  await fireEvent.press(screen.getByLabelText('粘贴为键入'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByText('逐字键入 240 字？')).toBeOnTheScreen();
  expect(
    rendered.core.calls.filter(call => call.method === 'sessions.text'),
  ).toHaveLength(1);
  await fireEvent.press(screen.getByTestId('clipboard-long-typing-confirm'));
  expect(rendered.core.calls).toContainEqual({
    method: 'sessions.text',
    args: [SID, 'x'.repeat(240)],
  });
});

it('reuses a host item by copying it to the phone', async () => {
  const rendered = await renderSheet();
  await fireEvent.press(screen.getByLabelText('再次使用：cargo run --release'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(rendered.platform.copied).toContain('cargo run --release');
  expect(rendered.ctx.showToast).toHaveBeenCalledWith('已复制到手机');
});

it('turning auto-sync off persists the setting and changes paused copy', async () => {
  const rendered = await renderSheet();
  await fireEvent.press(
    screen.getByLabelText('电脑上复制的内容自动同步到手机'),
  );
  expect(rendered.settingsStore.get('clipboard.autoSync')).toBe(false);
  expect(
    screen.getByText(/^已暂停同步 · 这是 \d\d:\d\d 的$/),
  ).toBeOnTheScreen();
});

it('disables both paste buttons for a viewer', async () => {
  await renderSheet(new FakePlatform(), record([fromHost128]), true);
  expect(screen.getByLabelText('粘贴到电脑')).toBeDisabled();
  expect(screen.getByLabelText('粘贴为键入')).toBeDisabled();
});

it('closes from the backdrop', async () => {
  const rendered = await renderSheet();
  await fireEvent.press(screen.getByLabelText('关闭剪贴板，回到画面'));
  expect(rendered.onClose).toHaveBeenCalledTimes(1);
});
