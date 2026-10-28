import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { adapters } from '../../i18n/zh/adapters';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { AdapterScreen } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { formatResolution } from '../format/latency';
import { MemoryKv } from '../storage/kv';
import AdapterSessionScreen from './AdapterSessionScreen';
import { resetAdapterLive } from './liveStore';
import { rememberedMonitors, rememberMonitors } from './monitorMemory';
import { WHOLE_DESKTOP } from './ScreenPicker';
import { installFakeAdapters } from './testing/fakeAdapters';
import type { FakeAdapters } from './testing/fakeAdapters';

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

const ID = '4611686018427387905';
const PRIMARY: AdapterScreen = { x: 0, y: 0, width: 1920, height: 1080 };
const SIDE: AdapterScreen = { x: 1920, y: 0, width: 1280, height: 1024 };
const clients: QueryClient[] = [];

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function mount(protocol: 'rdp' | 'vnc' = 'rdp', kv = new MemoryKv()) {
  const core = createFakeCore();
  const fake = installFakeAdapters(core);
  const route = mockRoute('AdapterSession', {
    target: { protocol, hostId: 'h1', addr: '100.64.0.9' },
    name: 'Windows PC',
    addrKind: 'tailnet',
  });
  const view = await renderWithProviders(
    <AdapterSessionScreen
      navigation={mockNavigation<'AdapterSession'>()}
      route={route}
    />,
    // The gesture coach has its own tests in AdapterSessionScreen.test.
    { core, kv, settings: { 'session.coachShown': true } },
  );
  clients.push(view.queryClient);
  await advance();
  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: ID,
      protocol,
      phase: 'streaming',
      credError: false,
      width: 3200,
      height: 1080,
    });
  });
  // The toolbar waits for the first picture (V12).
  await fireEvent(
    screen.getByTestId('remote-session-view-placeholder'),
    'firstFrame',
  );
  return { core, fake, kv };
}

async function emitScreens(fake: FakeAdapters, screens: AdapterScreen[]) {
  await act(async () => {
    fake.emitScreens(ID, screens);
  });
}

function selectCalls(fake: FakeAdapters) {
  return fake.calls.filter(call => call.method === 'selectScreen');
}

async function openPicker() {
  await fireEvent.press(screen.getByTestId('adapter-tb-screens'));
  expect(screen.getByTestId('adapter-screens')).toBeOnTheScreen();
}

function checked(index: number): boolean {
  return (
    screen.queryByTestId(`adapter-screen-${index}-check`, {
      includeHiddenElements: true,
    }) !== null
  );
}

function allChecked(): boolean {
  return (
    screen.queryByTestId('adapter-screen-all-check', {
      includeHiddenElements: true,
    }) !== null
  );
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  // A page unmounted on a live picture folds its session away (V33).
  resetAdapterLive();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.useRealTimers();
});

it('hidden_for_one_screen leaves the VNC screens button out', async () => {
  const { fake } = await mount('vnc');
  expect(screen.getByTestId('adapter-tb-mode')).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-tb-screens')).toBeNull();

  await emitScreens(fake, [PRIMARY]);
  expect(screen.getByTestId('adapter-tb-mode')).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-tb-screens')).toBeNull();
  expect(screen.queryByLabelText(adapters.a11y.screens(1))).toBeNull();
  expect(selectCalls(fake)).toEqual([]);
});

it('first_layout_selects_primary once and shows the button', async () => {
  const { fake } = await mount();
  await emitScreens(fake, [PRIMARY, SIDE]);

  expect(selectCalls(fake)).toEqual([
    { method: 'selectScreen', args: [ID, 0] },
  ]);
  expect(screen.getByLabelText(adapters.a11y.screens(2))).toBeOnTheScreen();

  await emitScreens(fake, [PRIMARY, { ...SIDE, width: 1920, height: 1080 }]);
  expect(selectCalls(fake)).toHaveLength(1);
  await openPicker();
  expect(screen.getByText(adapters.screens.title)).toBeOnTheScreen();
  expect(checked(0)).toBe(true);
  expect(checked(1)).toBe(false);
});

it('pick_second selects screen 2 and checks it', async () => {
  const { fake } = await mount();
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();

  await fireEvent.press(screen.getByText(adapters.screens.item(2)));
  expect(selectCalls(fake)).toEqual([
    { method: 'selectScreen', args: [ID, 0] },
    { method: 'selectScreen', args: [ID, 1] },
  ]);
  expect(screen.queryByTestId('adapter-screens')).toBeNull();

  await openPicker();
  expect(checked(1)).toBe(true);
  expect(checked(0)).toBe(false);
  expect(screen.getByTestId('adapter-screen-1')).toBeSelected();
  expect(screen.getByTestId('adapter-screen-0')).not.toBeSelected();

  await fireEvent.press(screen.getByTestId('adapter-screen-1'));
  expect(selectCalls(fake)).toHaveLength(2);
});

it('primary_label marks only the first screen as primary', async () => {
  const { fake } = await mount();
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();

  const first = screen.getByTestId('adapter-screen-0');
  const second = screen.getByTestId('adapter-screen-1');
  expect(first.props.accessibilityLabel).toContain(adapters.screens.item(1));
  expect(first.props.accessibilityLabel).toContain(adapters.screens.primary);
  expect(second.props.accessibilityLabel).toContain(adapters.screens.item(2));
  expect(second.props.accessibilityLabel).not.toContain(
    adapters.screens.primary,
  );
  expect(screen.getAllByText(adapters.screens.primary)).toHaveLength(1);
});

it('shrink_resets the selection without selecting again', async () => {
  const { fake } = await mount('vnc');
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();
  await fireEvent.press(screen.getByTestId('adapter-screen-1'));
  expect(selectCalls(fake)).toHaveLength(2);

  await openPicker();
  await emitScreens(fake, [PRIMARY]);
  expect(screen.queryByTestId('adapter-tb-screens')).toBeNull();
  expect(screen.queryByTestId('adapter-screens')).toBeNull();

  await emitScreens(fake, [PRIMARY, SIDE]);
  expect(screen.queryByTestId('adapter-screens')).toBeNull();
  await openPicker();
  expect(checked(0)).toBe(true);
  expect(checked(1)).toBe(false);
  expect(selectCalls(fake)).toHaveLength(2);
});

it('error_keeps_selection shows a toast when selecting fails', async () => {
  const { core, fake } = await mount();
  await emitScreens(fake, [PRIMARY, SIDE]);
  fake.failNext('selectScreen', new Error('screen index out of range'));
  await openPicker();

  await fireEvent.press(screen.getByTestId('adapter-screen-1'));
  expect(selectCalls(fake)).toHaveLength(2);
  // V20: the toast says the switch failed, not that the link broke.
  expect(screen.getByText(adapters.toast.screenSwitchFailed)).toBeOnTheScreen();
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['warn', 'adapters', 'select screen failed'],
  });

  await openPicker();
  expect(checked(0)).toBe(true);
  expect(checked(1)).toBe(false);
});

it('whole_desktop_value matches the Rust WHOLE_DESKTOP index', () => {
  expect(WHOLE_DESKTOP).toBe(255);
});

it('all_screens lists the whole desktop first and selects it', async () => {
  const { fake } = await mount();
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();

  const rows = screen.getAllByTestId(/^adapter-screen-(all|\d+)$/);
  expect(rows.map(row => row.props.testID)).toEqual([
    'adapter-screen-all',
    'adapter-screen-0',
    'adapter-screen-1',
  ]);
  const all = screen.getByTestId('adapter-screen-all');
  expect(all.props.accessibilityLabel).toContain(adapters.screens.all);
  expect(all.props.accessibilityLabel).toContain(formatResolution(3200, 1080));
  expect(allChecked()).toBe(false);
  expect(checked(0)).toBe(true);

  await fireEvent.press(screen.getByText(adapters.screens.all));
  expect(selectCalls(fake)).toEqual([
    { method: 'selectScreen', args: [ID, 0] },
    { method: 'selectScreen', args: [ID, WHOLE_DESKTOP] },
  ]);
  expect(screen.queryByTestId('adapter-screens')).toBeNull();

  await openPicker();
  expect(allChecked()).toBe(true);
  expect(checked(0)).toBe(false);
  expect(checked(1)).toBe(false);
  expect(screen.getByTestId('adapter-screen-all')).toBeSelected();
  expect(screen.getByTestId('adapter-screen-0')).not.toBeSelected();

  await fireEvent.press(screen.getByTestId('adapter-screen-all'));
  expect(selectCalls(fake)).toHaveLength(2);
});

it('single_then_all moves the check back to all displays', async () => {
  const { fake } = await mount();
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();
  await fireEvent.press(screen.getByTestId('adapter-screen-1'));
  await openPicker();
  expect(checked(1)).toBe(true);
  expect(allChecked()).toBe(false);

  await fireEvent.press(screen.getByTestId('adapter-screen-all'));
  expect(selectCalls(fake)).toEqual([
    { method: 'selectScreen', args: [ID, 0] },
    { method: 'selectScreen', args: [ID, 1] },
    { method: 'selectScreen', args: [ID, WHOLE_DESKTOP] },
  ]);
  await openPicker();
  expect(allChecked()).toBe(true);
  expect(checked(0)).toBe(false);
  expect(checked(1)).toBe(false);
  expect(screen.getByTestId('adapter-screen-all')).toBeSelected();
  expect(screen.getByTestId('adapter-screen-1')).not.toBeSelected();

  await fireEvent.press(screen.getByTestId('adapter-screen-0'));
  expect(selectCalls(fake)).toHaveLength(4);
  expect(selectCalls(fake)[3]).toEqual({
    method: 'selectScreen',
    args: [ID, 0],
  });
  await openPicker();
  expect(checked(0)).toBe(true);
  expect(allChecked()).toBe(false);
});

it('all_kept_across_layouts keeps the whole desktop checked', async () => {
  const { fake } = await mount('vnc');
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();
  await fireEvent.press(screen.getByTestId('adapter-screen-all'));

  await emitScreens(fake, [PRIMARY, { ...SIDE, width: 1920, height: 1080 }]);
  await emitScreens(fake, [PRIMARY]);
  expect(screen.queryByTestId('adapter-tb-screens')).toBeNull();
  await emitScreens(fake, [PRIMARY, SIDE]);
  await openPicker();
  expect(allChecked()).toBe(true);
  expect(checked(0)).toBe(false);
  expect(selectCalls(fake)).toHaveLength(2);
});

function label(index: number): string {
  return screen.getByTestId(`adapter-screen-${index}`).props.accessibilityLabel;
}

it('guessed_side_by_side names screens by side without a primary', async () => {
  const { fake } = await mount('vnc');
  await emitScreens(fake, [
    { x: 0, y: 0, width: 1920, height: 1080, guess: 'blackGap' },
    { x: 1920, y: 180, width: 1280, height: 720, guess: 'blackGap' },
  ]);
  // Like a reported layout, the first guessed one shows the top-left screen.
  expect(selectCalls(fake)).toEqual([
    { method: 'selectScreen', args: [ID, 0] },
  ]);
  await openPicker();

  expect(label(0)).toContain(adapters.screens.guessed.left);
  expect(label(1)).toContain(adapters.screens.guessed.right);
  expect(screen.queryByText(adapters.screens.item(1))).toBeNull();
  expect(screen.queryByText(adapters.screens.primary)).toBeNull();
  expect(screen.queryByText(adapters.screens.aspectNote)).toBeNull();
  expect(checked(0)).toBe(true);
});

it('guessed_stacked names top and bottom screens', async () => {
  const { fake } = await mount('vnc');
  await emitScreens(fake, [
    { x: 0, y: 0, width: 2560, height: 1440, guess: 'blackGap' },
    { x: 320, y: 1440, width: 1920, height: 1080, guess: 'blackGap' },
  ]);
  await openPicker();

  expect(label(0)).toContain(adapters.screens.guessed.top);
  expect(label(1)).toContain(adapters.screens.guessed.bottom);
});

it('guessed_three names the middle screen', async () => {
  const { fake } = await mount('vnc');
  await emitScreens(fake, [
    { x: 0, y: 200, width: 1280, height: 800, guess: 'blackGap' },
    { x: 1280, y: 0, width: 1920, height: 1080, guess: 'blackGap' },
    { x: 3200, y: 200, width: 1280, height: 800, guess: 'blackGap' },
  ]);
  await openPicker();

  expect(label(0)).toContain(adapters.screens.guessed.left);
  expect(label(1)).toContain(adapters.screens.guessed.middle);
  expect(label(2)).toContain(adapters.screens.guessed.right);
});

it('aspect_note says halves split by aspect may not line up', async () => {
  const { fake } = await mount('vnc');
  await emitScreens(fake, [
    { x: 0, y: 0, width: 1720, height: 1440, guess: 'aspect' },
    { x: 1720, y: 0, width: 1720, height: 1440, guess: 'aspect' },
  ]);
  await openPicker();

  expect(screen.getByText(adapters.screens.aspectNote)).toBeOnTheScreen();
  expect(label(0)).toContain(adapters.screens.guessed.left);
  expect(label(1)).toContain(adapters.screens.guessed.right);
  expect(screen.queryByText(adapters.screens.primary)).toBeNull();

  await fireEvent.press(screen.getByTestId('adapter-screen-all'));
  expect(selectCalls(fake)).toEqual([
    { method: 'selectScreen', args: [ID, 0] },
    { method: 'selectScreen', args: [ID, WHOLE_DESKTOP] },
  ]);
});

const RDP_TARGET = {
  protocol: 'rdp',
  hostId: 'h1',
  addr: '100.64.0.9',
} as const;

function monitorCalls(fake: FakeAdapters) {
  return fake.calls.filter(call => call.method === 'setMonitors');
}

describe('monitor count (RS-MON1)', () => {
  it('rdp_button_always_shows the count with one screen', async () => {
    const { fake } = await mount();
    expect(screen.getByLabelText(adapters.a11y.screens(1))).toBeOnTheScreen();
    await emitScreens(fake, [PRIMARY]);
    await openPicker();

    expect(screen.getByText(adapters.toolbar.screens)).toBeOnTheScreen();
    expect(screen.getByText(adapters.monitors.title)).toBeOnTheScreen();
    expect(screen.getByText(adapters.monitors.note)).toBeOnTheScreen();
    expect(screen.queryByTestId('adapter-screen-all')).toBeNull();
    expect(screen.getByTestId('adapter-monitors-1')).toBeSelected();
    expect(screen.getByTestId('adapter-monitors-2')).not.toBeSelected();
  });

  it('lists the screens above the count', async () => {
    const { fake } = await mount();
    await emitScreens(fake, [PRIMARY, SIDE]);
    await openPicker();

    expect(screen.getByText(adapters.screens.title)).toBeOnTheScreen();
    expect(screen.getByTestId('adapter-screen-1')).toBeOnTheScreen();
    expect(screen.getByTestId('adapter-monitors')).toBeOnTheScreen();
  });

  it('vnc_has_no_count even with several screens', async () => {
    const { fake } = await mount('vnc');
    await emitScreens(fake, [PRIMARY, SIDE]);
    await openPicker();

    expect(screen.getByTestId('adapter-screen-1')).toBeOnTheScreen();
    expect(screen.queryByTestId('adapter-monitors')).toBeNull();
  });

  it('opens with the count remembered for this computer', async () => {
    const kv = new MemoryKv();
    rememberMonitors(kv, RDP_TARGET, 3);
    const { fake } = await mount('rdp', kv);

    const open = fake.calls.find(call => call.method === 'open');
    expect(open?.args[1]).toMatchObject({ monitors: 3 });
    await openPicker();
    expect(screen.getByTestId('adapter-monitors-3')).toBeSelected();
  });

  it('opens VNC without a count', async () => {
    const { fake } = await mount('vnc');
    const open = fake.calls.find(call => call.method === 'open');
    expect(open?.args[1]).not.toHaveProperty('monitors');
  });

  it('pick_two asks for two monitors, remembers it and shows the primary', async () => {
    const { fake, kv } = await mount();
    const open = fake.calls.find(call => call.method === 'open');
    expect(open?.args[1]).toMatchObject({ monitors: 1 });
    await openPicker();

    await fireEvent.press(screen.getByTestId('adapter-monitors-2'));
    expect(monitorCalls(fake)).toEqual([
      { method: 'setMonitors', args: [ID, 2] },
    ]);
    expect(screen.queryByTestId('adapter-screens')).toBeNull();
    expect(rememberedMonitors(kv, RDP_TARGET)).toBe(2);

    // The new layout shows the primary monitor first (C13.6).
    await emitScreens(fake, [PRIMARY, SIDE]);
    expect(selectCalls(fake)).toEqual([
      { method: 'selectScreen', args: [ID, 0] },
    ]);
    await openPicker();
    expect(screen.getByTestId('adapter-monitors-2')).toBeSelected();
    await fireEvent.press(screen.getByTestId('adapter-monitors-2'));
    expect(monitorCalls(fake)).toHaveLength(1);
  });

  it('reconnect_hint names the new count while the session reconnects', async () => {
    const { core, fake } = await mount();
    await openPicker();
    await fireEvent.press(screen.getByTestId('adapter-monitors-2'));

    const state = (phase: 'authenticating' | 'streaming') => ({
      type: 'adapter.state' as const,
      sessionId: ID,
      protocol: 'rdp' as const,
      phase,
      credError: false,
      ...(phase === 'streaming' ? { width: 3200, height: 1080 } : {}),
    });
    await act(async () => {
      core.emit(state('authenticating'));
    });
    expect(
      screen.getByText(adapters.monitors.reconnecting(2)),
    ).toBeOnTheScreen();
    expect(screen.queryByText(adapters.authenticating)).toBeNull();

    await act(async () => {
      core.emit(state('streaming'));
    });
    await emitScreens(fake, [PRIMARY, SIDE]);
    expect(screen.queryByText(adapters.monitors.reconnecting(2))).toBeNull();
    await act(async () => {
      core.emit(state('authenticating'));
    });
    expect(screen.getByText(adapters.authenticating)).toBeOnTheScreen();
  });

  it('fewer_monitors goes back to the primary without a notice', async () => {
    const kv = new MemoryKv();
    rememberMonitors(kv, RDP_TARGET, 2);
    const { fake } = await mount('rdp', kv);
    await emitScreens(fake, [PRIMARY, SIDE]);
    await openPicker();
    await fireEvent.press(screen.getByTestId('adapter-screen-1'));
    await openPicker();

    await fireEvent.press(screen.getByTestId('adapter-monitors-1'));
    expect(monitorCalls(fake)).toEqual([
      { method: 'setMonitors', args: [ID, 1] },
    ]);
    await emitScreens(fake, [PRIMARY]);
    expect(screen.queryByText(adapters.toast.screenGone)).toBeNull();
  });

  it('error_keeps_count shows a toast when the change fails', async () => {
    const { core, fake, kv } = await mount();
    fake.failNext('setMonitors', new Error('unknown adapter session'));
    await openPicker();

    await fireEvent.press(screen.getByTestId('adapter-monitors-2'));
    expect(screen.getByText(adapters.monitors.changeFailed)).toBeOnTheScreen();
    expect(core.calls).toContainEqual({
      method: 'logs.write',
      args: ['warn', 'adapters', 'set monitors failed'],
    });
    expect(rememberedMonitors(kv, RDP_TARGET)).toBe(1);
    await openPicker();
    expect(screen.getByTestId('adapter-monitors-1')).toBeSelected();
  });
});
