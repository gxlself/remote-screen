import { act, screen } from '@testing-library/react-native';
import { useQueryClient } from '@tanstack/react-query';
import {
  useSafeAreaFrame,
  useSafeAreaInsets,
} from 'react-native-safe-area-context';
import AppText from '../components/AppText';
import { useCore } from '../native/rscore/CoreContext';
import { createFakeCore } from '../native/rscore/fakeCore';
import { usePlatform } from '../native/platform/PlatformContext';
import { FakePlatform } from '../native/platform/fakePlatform';
import { LOCAL_REACH_KEY } from '../features/data/localReach';
import { MemoryKv } from '../features/storage/kv';
import { computers } from '../native/rscore/fixtures';
import { useKv } from '../features/storage/StorageContext';
import { useSetting } from '../features/settings/store';
import { useCoreEvent } from '../features/events/EventsProvider';
import { mockNavigation, mockRoute } from './navigation';
import { renderWithProviders } from './render';

it('provides the injected services, settings, safe area and event bus', async () => {
  const core = createFakeCore();
  const platform = new FakePlatform();
  const kv = new MemoryKv();
  const received = jest.fn();
  const read = jest.fn();
  function Probe() {
    const [hud] = useSetting('session.hud');
    useCoreEvent('network.changed', received);
    read({
      core: useCore(),
      platform: usePlatform(),
      kv: useKv(),
      queryClient: useQueryClient(),
      frame: useSafeAreaFrame(),
      insets: useSafeAreaInsets(),
    });
    return <AppText testID="hud">{String(hud)}</AppText>;
  }

  const rendered = await renderWithProviders(<Probe />, {
    core,
    platform,
    kv,
    settings: { 'session.hud': true },
  });

  expect(screen.getByTestId('hud')).toHaveTextContent('true');
  expect(rendered.core).toBe(core);
  expect(rendered.platform).toBe(platform);
  expect(rendered.kv).toBe(kv);
  expect(rendered.settingsStore.get('session.hud')).toBe(true);
  expect(kv.getJSON('settings.session.hud')).toBe(true);
  expect(read).toHaveBeenLastCalledWith({
    core,
    platform,
    kv,
    queryClient: rendered.queryClient,
    frame: { x: 0, y: 0, width: 390, height: 844 },
    insets: { top: 47, bottom: 34, left: 0, right: 0 },
  });
  const event = { type: 'network.changed' as const, status: { ifaces: [] } };
  await act(() => core.emit(event));
  expect(received).toHaveBeenCalledWith(event);
  expect(core.subscriberCount()).toBe(1);
  await rendered.unmount();
  expect(core.subscriberCount()).toBe(0);
  rendered.queryClient.clear();
});

it('creates independent default services and uses test query defaults', async () => {
  const first = await renderWithProviders(<></>);
  const second = await renderWithProviders(<></>);
  expect(first.core).not.toBe(second.core);
  expect(first.platform).not.toBe(second.platform);
  expect(first.kv).not.toBe(second.kv);
  expect(first.queryClient).not.toBe(second.queryClient);
  expect(first.settingsStore.get('session.hud')).toBe(false);
  expect(first.queryClient.getDefaultOptions().queries).toMatchObject({
    retry: false,
    gcTime: Infinity,
    staleTime: 0,
  });
  expect(first.queryClient.getDefaultOptions().mutations).toMatchObject({
    gcTime: Infinity,
  });
});

it('writes no Hub address by default', async () => {
  const view = await renderWithProviders(<></>);
  expect(view.kv.getString('settings.hub.url')).toBeUndefined();
});

it('seeds the local directory only when asked', async () => {
  const plain = await renderWithProviders(<></>, {
    core: createFakeCore({ pairedHosts: [] }),
  });
  await expect(plain.core.hosts.paired()).resolves.toEqual([]);

  const seeded = await renderWithProviders(<></>, {
    core: createFakeCore({ pairedHosts: [] }),
    localDirectory: { computers: computers.slice(0, 1) },
  });
  const paired = await seeded.core.hosts.paired();
  expect(paired.map(host => host.hostId)).toEqual([computers[0].hostId]);
  expect(seeded.kv.getJSON(LOCAL_REACH_KEY)).toEqual({
    [computers[0].hostId]: {
      online: computers[0].online && computers[0].hostRunning,
      at: computers[0].lastSeenAt,
    },
  });
});

it('records navigation and creates typed routes and parent mocks', () => {
  const navigation = mockNavigation<'DeviceSheet'>();
  navigation.navigate('MainTabs', { screen: 'Home' });
  expect(navigation.navigate).toHaveBeenCalledWith('MainTabs', {
    screen: 'Home',
  });
  // Home belongs to MainTabs in U0; the mock can still record an untyped call.
  (navigation.navigate as jest.Mock)('Home');
  expect(navigation.navigate).toHaveBeenLastCalledWith('Home');
  expect(navigation.canGoBack()).toBe(true);
  const unsubscribe = navigation.addListener('focus', jest.fn());
  expect(typeof unsubscribe).toBe('function');
  unsubscribe();
  const parent = navigation.getParent();
  expect(navigation.getParent()).toBe(parent);
  parent?.goBack();
  expect(parent?.goBack).toHaveBeenCalledTimes(1);
  expect(mockRoute('DeviceSheet', { hostId: 'h1' })).toEqual({
    key: 'DeviceSheet-test',
    name: 'DeviceSheet',
    params: { hostId: 'h1' },
  });
  expect(mockRoute('SshKeys').params).toBeUndefined();
});
