import { cleanup, fireEvent } from '@testing-library/react-native';
import { deriveSessionView } from '../../features/session/view';
import type { SessionRecord } from '../../features/session/store';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import { IpadShortcutKeys } from './IpadShortcutKeys';
import type { SessionCtx } from './slots';

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
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
    clipboard: [],
    ...overrides,
  };
}

function context(overrides: Partial<SessionCtx> = {}): SessionCtx {
  const rec = overrides.rec ?? record();
  const os = overrides.os ?? 'macos';
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os,
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
    rec,
    view: deriveSessionView(rec, { os, quality: 'auto', myDeviceId: null }),
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: null,
      fps: null,
      tcp: false,
      alertText: null,
      a11y: '',
    },
    isPad: true,
    reduceMotion: false,
    mode: 'touch',
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
    ...overrides,
  };
}

async function mount(
  ctx: SessionCtx,
  opts: {
    core?: FakeCore;
    platform?: FakePlatform;
    settings?: { 'session.haptics'?: boolean };
  } = {},
) {
  const core = opts.core ?? createFakeCore();
  const platform = opts.platform ?? new FakePlatform();
  const view = await renderWithProviders(<IpadShortcutKeys ctx={ctx} />, {
    core,
    platform,
    settings: opts.settings,
  });
  return { ...view, core, platform };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('lays out the Mac shortcut group with canvas labels', async () => {
  const view = await mount(context());

  expect(
    view.getByLabelText('iPad 会拦截的快捷键，点一下发给 Mac'),
  ).toHaveStyle({ flexDirection: 'row', gap: 4 });
  expect(view.getByLabelText('发送 Command-Tab 给 Mac')).toHaveStyle({
    height: 44,
    paddingHorizontal: 10,
    borderRadius: 22,
  });
});

it('sends Command-Tab and Command-Space with light haptics', async () => {
  const platform = new FakePlatform();
  const view = await mount(context(), { platform });

  await fireEvent.press(view.getByLabelText('发送 Command-Tab 给 Mac'));
  await fireEvent.press(view.getByLabelText('发送 Command-空格 给 Mac'));

  expect(view.core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-live-1', 'cmdTab'],
  });
  expect(view.core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-live-1', 'cmdSpace'],
  });
  expect(platform.calls).toContainEqual({ method: 'haptic', args: ['light'] });
});

it('disables the unsupported globe key and does not send it', async () => {
  const view = await mount(context());
  const globe = view.getByLabelText('发送地球键给 Mac，暂不支持');

  expect(globe.props.accessibilityState.disabled).toBe(true);
  const icon = view.getByTestId('ipad-globe', { includeHiddenElements: true });
  expect(icon).toHaveProp('width', 18);
  expect(icon).toHaveProp('height', 18);
  await fireEvent.press(globe);

  expect(view.core.calls).not.toContainEqual(
    expect.objectContaining({ method: 'sessions.combo' }),
  );
  expect(view.platform.calls).not.toContainEqual(
    expect.objectContaining({ method: 'haptic' }),
  );
});

it('skips haptics when the setting is off', async () => {
  const platform = new FakePlatform();
  const view = await mount(context(), {
    platform,
    settings: { 'session.haptics': false },
  });

  await fireEvent.press(view.getByLabelText('发送 Command-Tab 给 Mac'));

  expect(view.core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-live-1', 'cmdTab'],
  });
  expect(platform.calls).not.toContainEqual(
    expect.objectContaining({ method: 'haptic' }),
  );
});

it('disables every key while watching', async () => {
  const view = await mount(context({ rec: record({ role: 'viewer' }) }));

  expect(
    view.getByLabelText('发送 Command-Tab 给 Mac，观看时不可用'),
  ).toBeDisabled();
  expect(
    view.getByLabelText('发送 Command-空格 给 Mac，观看时不可用'),
  ).toBeDisabled();
  expect(
    view.getByLabelText('发送地球键给 Mac，暂不支持，观看时不可用'),
  ).toBeDisabled();
});

it('disables every key without a session id', async () => {
  const view = await mount(context({ sessionId: null }));
  const first = view.getByLabelText('发送 Command-Tab 给 Mac');

  expect(first.props.accessibilityState.disabled).toBe(true);
  expect(first).toHaveStyle({ opacity: 0.4 });
  await fireEvent.press(first);
  expect(view.core.calls).not.toContainEqual(
    expect.objectContaining({ method: 'sessions.combo' }),
  );
});

it('uses the Windows wording and shortcuts', async () => {
  const view = await mount(
    context({ os: 'windows', rec: record({ hostName: '工位 Windows' }) }),
  );

  expect(
    view.getByLabelText('iPad 会拦截的快捷键，点一下发给电脑'),
  ).toBeOnTheScreen();
  expect(view.getByLabelText('发送 Alt-Tab 给电脑')).toBeOnTheScreen();
  expect(view.getByLabelText('发送 Windows 键给电脑')).toBeOnTheScreen();

  await fireEvent.press(view.getByLabelText('发送 Alt-Tab 给电脑'));
  await fireEvent.press(view.getByLabelText('发送 Windows 键给电脑'));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-live-1', 'altTab'],
  });
  expect(view.core.calls).toContainEqual({
    method: 'sessions.combo',
    args: ['s-live-1', 'winKey'],
  });
});

it('renders nothing outside an iPad', async () => {
  const view = await mount(context({ isPad: false }));

  expect(view.toJSON()).toBeNull();
});
