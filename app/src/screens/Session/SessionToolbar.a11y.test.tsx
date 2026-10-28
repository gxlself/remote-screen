import { cleanup } from '@testing-library/react-native';
import { PixelRatio, Platform, StyleSheet } from 'react-native';
import { deriveSessionView } from '../../features/session/view';
import { hudModel } from '../../features/session/hud';
import type { SessionRecord } from '../../features/session/store';
import { liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import { FollowZoomChip } from './FollowZoomChip';
import { IpadShortcutKeys } from './IpadShortcutKeys';
import { SessionHud } from './SessionHud';
import { SessionToolbar } from './SessionToolbar';
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
    hud: hudModel(rec),
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

async function mountToolbar(ctx: SessionCtx) {
  return renderWithProviders(<SessionToolbar ctx={ctx} />);
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('uses the platform hit target for toolbar buttons', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const view = await mountToolbar(context({ os: 'windows' }));
  const expected = Math.max(0, (hit.min - 44) / 2);

  expect(view.getByTestId('toolbar-ctrl-alt-del')).toHaveProp('hitSlop', {
    top: expected,
    bottom: expected,
  });
  expect(view.getByTestId('toolbar-quality')).toHaveProp('hitSlop', {
    top: expected,
    bottom: expected,
  });
});

it('switches to a scrolling toolbar and removes fixed height at large text', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await mountToolbar(context({ os: 'windows' }));

  expect(view.getByTestId('toolbar-ctrl-alt-del')).toHaveStyle({
    minHeight: 44,
  });
  const scroll = view.getByTestId('toolbar-ctrl-alt-del').parent?.parent;
  expect(scroll).toHaveProp('horizontal', true);
  expect(scroll).toHaveProp(
    'contentContainerStyle',
    expect.objectContaining({ flexDirection: 'row', gap: 4, padding: 4 }),
  );
});

it('keeps iPad shortcut buttons individually accessible with hints', async () => {
  const view = await renderWithProviders(
    <IpadShortcutKeys ctx={context({ isPad: true })} />,
  );
  const group = view.getByLabelText('iPad 会拦截的快捷键，点一下发给 Mac');
  expect(group).not.toHaveProp('accessible', true);
  expect(
    view.getByRole('button', { name: '发送 Command-Tab 给 Mac' }),
  ).toHaveProp('accessibilityHint', 'iPad 会拦截的快捷键，点一下发给 Mac');
  expect(view.getByTestId('ipad-key-cmdTab')).toHaveProp('hitSlop', {
    top: Math.max(0, (hit.min - 44) / 2),
    bottom: Math.max(0, (hit.min - 44) / 2),
  });
});

it('keeps HUD and follow zoom labels usable at large text', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const ctx = context();
  const view = await renderWithProviders(
    <>
      <SessionHud ctx={ctx} />
      <FollowZoomChip sessionId={ctx.sessionId} />
    </>,
  );

  expect(view.getByTestId('follow-zoom-chip')).toHaveStyle({
    minHeight: 30,
  });
  expect(view.getByTestId('session-hud-pill').parent?.parent).toHaveStyle({
    padding: 6,
  });
  const hudLayer = view.getByTestId('session-hud-pill').parent?.parent?.parent;
  const hudStyle = StyleSheet.flatten(hudLayer?.props.style);
  expect(hudStyle.top + 6).toBe(12);
});

it('uses a minimum width for the stats overlay at large text', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await renderWithProviders(<SessionHud ctx={context()} />, {
    settings: { 'session.hud': true },
  });
  expect(view.getByTestId('session-stats')).toHaveStyle({ minWidth: 168 });
});
