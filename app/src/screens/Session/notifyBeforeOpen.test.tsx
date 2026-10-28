import { act, cleanup } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { NOTIFY_ASKED_KEY } from '../../features/session/notifyPermission';
import { MemoryKv } from '../../features/storage/kv';
import type { KvStore } from '../../features/storage/kv';
import { resetKeyboardInset } from '../../native/keyboard/keyboardInset';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionScreen from './index';

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

const hostId = 'h7v2m9q3k8x4tb1a';
const clients: QueryClient[] = [];

function androidPlatform(osVersion = '33'): FakePlatform {
  const platform = new FakePlatform();
  platform.info.platform = 'android';
  platform.info.osVersion = osVersion;
  return platform;
}

async function mount(platform: FakePlatform, kv: KvStore, core: FakeCore) {
  const navigation = mockNavigation<'Session'>();
  const route = mockRoute('Session', { hostId });
  const view = await renderWithProviders(
    <SessionScreen navigation={navigation} route={route} />,
    { core, kv, platform },
  );
  clients.push(view.queryClient);
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, navigation };
}

function trackRecords(platform: FakePlatform, core: FakeCore): string[] {
  const records: string[] = [];
  const requestNotifications = platform.requestNotifications.bind(platform);
  jest.spyOn(platform, 'requestNotifications').mockImplementation(async () => {
    const result = await requestNotifications();
    records.push(`platform.${platform.calls.at(-1)?.method ?? 'none'}`);
    return result;
  });
  const open = core.sessions.open.bind(core.sessions);
  jest.spyOn(core.sessions, 'open').mockImplementation(async (id, opts) => {
    const sessionId = await open(id, opts);
    records.push(`core.${core.calls.at(-1)?.method ?? 'none'}`);
    return sessionId;
  });
  return records;
}

beforeEach(() => {
  jest.useFakeTimers();
  resetKeyboardInset();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('requests notification permission before opening a session on Android 13', async () => {
  const platform = androidPlatform();
  const kv = new MemoryKv();
  const core = createFakeCore({ sessions: [] });
  const records = trackRecords(platform, core);

  await mount(platform, kv, core);

  expect(
    platform.calls.filter(call => call.method === 'requestNotifications'),
  ).toHaveLength(1);
  expect(
    core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
  expect(records).toEqual([
    'platform.requestNotifications',
    'core.sessions.open',
  ]);
  expect(kv.getString(NOTIFY_ASKED_KEY)).toBe('1');
});

it('opens the session when notification permission is denied', async () => {
  const platform = androidPlatform();
  const core = createFakeCore({ sessions: [] });
  jest.spyOn(platform, 'requestNotifications').mockResolvedValue('denied');

  await mount(platform, new MemoryKv(), core);

  expect(platform.requestNotifications).toHaveBeenCalledTimes(1);
  expect(
    core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
});

it('does not request again when a second session screen uses the same KV', async () => {
  const platform = androidPlatform();
  const kv = new MemoryKv();
  const first = await mount(platform, kv, createFakeCore({ sessions: [] }));
  expect(
    platform.calls.filter(call => call.method === 'requestNotifications'),
  ).toHaveLength(1);
  await first.unmount();

  const second = await mount(platform, kv, createFakeCore({ sessions: [] }));

  expect(
    platform.calls.filter(call => call.method === 'requestNotifications'),
  ).toHaveLength(1);
  expect(
    second.core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
});

it('opens directly on iOS without requesting notification permission', async () => {
  const platform = new FakePlatform();
  const core = createFakeCore({ sessions: [] });

  await mount(platform, new MemoryKv(), core);

  expect(
    platform.calls.filter(call => call.method === 'requestNotifications'),
  ).toEqual([]);
  expect(
    core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(1);
});

it('does not open a session when the user leaves during the permission prompt', async () => {
  const platform = androidPlatform();
  const core = createFakeCore({ sessions: [] });
  let answer: (value: 'granted' | 'denied') => void = () => undefined;
  jest.spyOn(platform, 'requestNotifications').mockImplementation(
    () =>
      new Promise(resolve => {
        answer = resolve;
      }),
  );

  const view = await mount(platform, new MemoryKv(), core);
  await act(async () => {
    await view.unmount();
  });
  await act(async () => {
    answer('granted');
    await jest.advanceTimersByTimeAsync(1);
  });

  expect(
    core.calls.filter(call => call.method === 'sessions.open'),
  ).toHaveLength(0);
});
