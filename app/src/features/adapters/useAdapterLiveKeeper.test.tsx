import { act, cleanup } from '@testing-library/react-native';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type {
  AdapterErrorInfo,
  AdapterTarget,
  CoreEvent,
  SessionId,
} from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { readHistory } from '../sessions/history';
import { adapterLive, resetAdapterLive } from './liveStore';
import type { CollapsedAdapter } from './liveStore';
import type { CollapsedKeep } from '../settings/schema';
import { LOST_IN_BACKGROUND } from './logic';
import { installFakeAdapters } from './testing/fakeAdapters';
import type { FakeAdapters } from './testing/fakeAdapters';
import {
  COLLAPSED_KEEP,
  COLLAPSED_KEEP_MS,
  RECLAIMED,
  useAdapterLiveKeeper,
  useDisconnectCollapsed,
} from './useAdapterLiveKeeper';

const vnc: AdapterTarget = { protocol: 'vnc', hostId: 'h1', addr: '100.64.0.7' };
const DROP: AdapterErrorInfo = {
  code: 'timeout',
  rsCode: ADAPTER_RS_E.unreachable,
  message: 'link lost',
};

let disconnect: ((id: SessionId) => void) | undefined;

function Keeper() {
  useAdapterLiveKeeper();
  disconnect = useDisconnectCollapsed();
  return null;
}

function record(id: SessionId): CollapsedAdapter {
  const nowUs = Date.now() * 1000;
  return {
    sessionId: id,
    params: { target: vnc, name: 'Mac mini', addrKind: 'tailnet' },
    name: 'Mac mini',
    phase: 'streaming',
    error: null,
    width: 1920,
    height: 1080,
    screens: [],
    screenIndex: 0,
    mode: 'trackpad',
    startedAtUs: nowUs - 60_000_000,
    collapsedAtUs: nowUs,
    history: {
      live: {
        id: `adapter:${id}`,
        kind: 'adapter',
        targetId: 'host:h1',
        name: 'Mac mini',
        startedAtUs: nowUs - 60_000_000,
        protocol: 'vnc',
      },
      stoppedAtUs: null,
      error: null,
      name: 'Mac mini',
      targetId: 'host:h1',
    },
  };
}

async function setup(keep?: CollapsedKeep) {
  const core = createFakeCore();
  const fake = installFakeAdapters(core);
  // The keeper only looks after sessions the native side has open.
  const id = await fake.open(vnc, { viewWidth: 1, viewHeight: 1, scale: 1 });
  adapterLive.put(record(id));
  const view = await renderWithProviders(<Keeper />, {
    core,
    ...(keep === undefined
      ? {}
      : { settings: { 'session.collapsedKeep': keep } }),
  });
  return { ...view, core, fake, id };
}

async function emit(core: FakeCore, event: CoreEvent) {
  await act(async () => {
    core.emit(event);
    await jest.advanceTimersByTimeAsync(1);
  });
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

function closes(fake: FakeAdapters) {
  return fake.calls.filter(call => call.method === 'close');
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  resetAdapterLive();
  disconnect = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('follows a folded-away session and keeps one that dropped', async () => {
  const { core, id } = await setup();

  await emit(core, {
    type: 'adapter.screens',
    sessionId: id,
    screens: [{ x: 0, y: 0, width: 1280, height: 800 }],
  });
  expect(adapterLive.peek(id)?.screens).toHaveLength(1);

  await emit(core, {
    type: 'adapter.state',
    sessionId: id,
    protocol: 'vnc',
    phase: 'ended',
    credError: false,
    error: DROP,
  });
  // Still listed, so returning to it can connect again (or explain).
  expect(adapterLive.peek(id)).toMatchObject({
    phase: 'ended',
    error: DROP,
    history: { error: DROP, stoppedAtUs: expect.any(Number) },
  });
});

it('a clean end leaves nothing to return to and writes 最近', async () => {
  const { core, id, kv } = await setup();

  await emit(core, {
    type: 'adapter.state',
    sessionId: id,
    protocol: 'vnc',
    phase: 'ended',
    credError: false,
  });

  expect(adapterLive.peek(id)).toBeUndefined();
  expect(readHistory(kv)).toEqual([
    expect.objectContaining({ kind: 'adapter', name: 'Mac mini' }),
  ]);
});

it('closes a session folded away for too long', async () => {
  const { core, fake, id, kv } = await setup();

  await advance(COLLAPSED_KEEP_MS - 1000);
  expect(closes(fake)).toEqual([]);
  expect(adapterLive.peek(id)?.phase).toBe('streaming');

  await advance(1000);
  expect(closes(fake)).toEqual([{ method: 'close', args: [id] }]);
  expect(adapterLive.peek(id)).toMatchObject({
    phase: 'ended',
    error: RECLAIMED,
    // Our own clean-up is no reason to put in 最近.
    history: { error: null },
  });

  // The end our close causes does not take the record away.
  await emit(core, {
    type: 'adapter.state',
    sessionId: id,
    protocol: 'vnc',
    phase: 'ended',
    credError: false,
  });
  expect(adapterLive.peek(id)?.error).toEqual(RECLAIMED);
  expect(readHistory(kv)).toEqual([]);
});

it('notices a session the system killed while the app slept', async () => {
  const listeners: ((state: AppStateStatus) => void)[] = [];
  jest
    .spyOn(AppState, 'addEventListener')
    .mockImplementation((type, listener) => {
      if (type === 'change') {
        listeners.push(listener as (state: AppStateStatus) => void);
      }
      return { remove: jest.fn() };
    });
  const { fake, id } = await setup();
  const foreground = async () => {
    await act(async () => {
      for (const listener of listeners) {
        listener('active');
      }
    });
  };

  await foreground();
  expect(adapterLive.peek(id)?.phase).toBe('streaming');

  fake.dropSession(id);
  await foreground();
  expect(adapterLive.peek(id)).toMatchObject({
    phase: 'ended',
    error: LOST_IN_BACKGROUND,
  });
});

it('disconnecting closes the session and writes 最近', async () => {
  const { fake, id, kv } = await setup();

  await act(async () => disconnect?.(id));

  expect(closes(fake)).toEqual([{ method: 'close', args: [id] }]);
  expect(adapterLive.peek(id)).toBeUndefined();
  expect(readHistory(kv)).toEqual([
    expect.objectContaining({ kind: 'adapter', durationS: 60 }),
  ]);
});

it('disconnecting one that already ended closes nothing', async () => {
  const { fake, id } = await setup();
  await act(async () => adapterLive.end(id, DROP));

  await act(async () => disconnect?.(id));

  expect(closes(fake)).toEqual([]);
  expect(adapterLive.peek(id)).toBeUndefined();
});

describe('kept time (RS-MULTI1)', () => {
  const MINUTE = 60 * 1000;

  it('keeps folded-away sessions two hours by default', () => {
    expect(COLLAPSED_KEEP_MS).toBe(120 * MINUTE);
    expect(COLLAPSED_KEEP).toEqual({
      '10m': 10 * MINUTE,
      '30m': 30 * MINUTE,
      '2h': 120 * MINUTE,
      always: null,
    });
  });

  it.each([
    ['10m', 10 * MINUTE],
    ['30m', 30 * MINUTE],
    ['2h', 120 * MINUTE],
  ] as const)('%s closes it after that long', async (keep, ms) => {
    const { fake, id } = await setup(keep);

    await advance(ms - 1000);
    expect(closes(fake)).toEqual([]);
    await advance(1000);
    expect(closes(fake)).toEqual([{ method: 'close', args: [id] }]);
    expect(adapterLive.peek(id)?.error).toEqual(RECLAIMED);
  });

  it('一直保持 never closes it, even back from the background', async () => {
    const listeners: ((state: AppStateStatus) => void)[] = [];
    jest
      .spyOn(AppState, 'addEventListener')
      .mockImplementation((type, listener) => {
        if (type === 'change') {
          listeners.push(listener as (state: AppStateStatus) => void);
        }
        return { remove: jest.fn() };
      });
    const { fake, id } = await setup('always');

    await advance(48 * 60 * MINUTE);
    await act(async () => {
      for (const listener of listeners) {
        listener('active');
      }
    });
    expect(closes(fake)).toEqual([]);
    expect(adapterLive.peek(id)?.phase).toBe('streaming');
  });

  it('takes a paused session out of the pause before closing it', async () => {
    const { core, fake, id } = await setup('10m');
    const background = jest.spyOn(core.sessions, 'background');
    const close = jest.spyOn(fake, 'close');

    await advance(10 * MINUTE);
    expect(background).toHaveBeenCalledWith(id, false);
    expect(close).toHaveBeenCalledWith(id);
    expect(background.mock.invocationCallOrder[0]).toBeLessThan(
      close.mock.invocationCallOrder[0],
    );
  });

  it('does the same when the user disconnects it', async () => {
    const { core, fake, id } = await setup();
    const background = jest.spyOn(core.sessions, 'background');
    const close = jest.spyOn(fake, 'close');

    await act(async () => disconnect?.(id));
    expect(background).toHaveBeenCalledWith(id, false);
    expect(background.mock.invocationCallOrder[0]).toBeLessThan(
      close.mock.invocationCallOrder[0],
    );
  });
});
