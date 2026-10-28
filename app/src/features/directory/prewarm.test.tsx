import type { ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react-native';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { CoreProvider } from '../../native/rscore/CoreContext';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { MemoryKv } from '../storage/kv';
import { SettingsProvider, createSettingsStore } from '../settings/store';
import type { SettingsStore } from '../settings/store';
import type { CardModel } from './cardModel';
import { pickPrewarm, usePrewarm } from './prewarm';

function card(hostId: string, overrides: Partial<CardModel> = {}): CardModel {
  return {
    hostId,
    group: 1,
    tap: { kind: 'open' },
    thumbnailUrl: null,
    name: hostId,
    look: 'live',
    ...overrides,
  };
}

function wrapper(
  core: ReturnType<typeof createFakeCore>,
  settings: SettingsStore,
) {
  return ({ children }: { children: ReactNode }) => (
    <CoreProvider core={core}>
      <SettingsProvider store={settings}>{children}</SettingsProvider>
    </CoreProvider>
  );
}

function prewarmCalls(core: ReturnType<typeof createFakeCore>) {
  return core.calls.filter(call => call.method === 'sessions.prewarm');
}

afterEach(() => {
  jest.restoreAllMocks();
});

function spyOnAppState(): ((state: AppStateStatus) => void)[] {
  const listeners: ((state: AppStateStatus) => void)[] = [];
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_type, cb) => {
    listeners.push(cb as (state: AppStateStatus) => void);
    return { remove: jest.fn() };
  });
  return listeners;
}

describe('pickPrewarm', () => {
  it('takes up to four connectable cards from groups 0 and 1', () => {
    const cards = [
      card('session', { group: 0 }),
      card('update', { group: 1, tap: { kind: 'error', code: 170 } }),
      card('a'),
      card('hostdown', { group: 2 }),
      card('b'),
      card('c'),
      card('sleeping', { group: 3 }),
      card('d'),
      card('e'),
    ];
    expect(pickPrewarm(cards)).toEqual(['session', 'a', 'b', 'c']);
  });

  it('honours a custom limit and returns no ids for an empty list', () => {
    expect(pickPrewarm([card('a'), card('b')], 1)).toEqual(['a']);
    expect(pickPrewarm([])).toEqual([]);
  });
});

describe('usePrewarm', () => {
  it('calls prewarm once and skips an identical rerender', async () => {
    const core = createFakeCore();
    const settings = createSettingsStore(new MemoryKv());
    const { rerender } = await renderHook(
      ({ ids }: { ids: string[] }) => usePrewarm(ids),
      {
        wrapper: wrapper(core, settings),
        initialProps: { ids: ['a', 'b'] },
      },
    );
    expect(prewarmCalls(core)).toHaveLength(1);
    expect(prewarmCalls(core)[0]?.args).toEqual([['a', 'b']]);

    await rerender({ ids: ['b', 'a'] });
    expect(prewarmCalls(core)).toHaveLength(1);

    await rerender({ ids: ['a', 'b', 'c'] });
    expect(prewarmCalls(core)).toHaveLength(2);
  });

  it('does not call prewarm when the setting is disabled', async () => {
    const core = createFakeCore();
    const settings = createSettingsStore(new MemoryKv());
    settings.set('prewarm.enabled', false);
    await renderHook(() => usePrewarm(['a', 'b']), {
      wrapper: wrapper(core, settings),
    });
    expect(prewarmCalls(core)).toEqual([]);
  });

  it('does not call prewarm for an empty host list', async () => {
    const core = createFakeCore();
    const settings = createSettingsStore(new MemoryKv());
    await renderHook(() => usePrewarm([]), {
      wrapper: wrapper(core, settings),
    });
    expect(prewarmCalls(core)).toEqual([]);
  });

  it('calls again after returning from the background', async () => {
    const listeners = spyOnAppState();
    const core = createFakeCore();
    const settings = createSettingsStore(new MemoryKv());
    await renderHook(() => usePrewarm(['a', 'b']), {
      wrapper: wrapper(core, settings),
    });
    expect(prewarmCalls(core)).toHaveLength(1);

    await act(() => {
      for (const listener of listeners) {
        listener('background');
        listener('active');
      }
    });
    expect(prewarmCalls(core)).toHaveLength(2);
  });
});
