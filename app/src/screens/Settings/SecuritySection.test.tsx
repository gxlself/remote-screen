import { act, cleanup, screen } from '@testing-library/react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { MemoryKv } from '../../features/storage/kv';
import { settings as copy } from '../../i18n/zh/settings';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SecuritySection from './SecuritySection';

async function renderSection(kv = new MemoryKv()) {
  const core = createFakeCore();
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Settings'>['navigation'];
  await renderWithProviders(<SecuritySection navigation={navigation} />, {
    core,
    kv,
  });
  // Let the paired-host and key queries settle so their updates land in act.
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 0));
    });
  }
  return core;
}

afterEach(async () => {
  await cleanup();
});

it('lists only the entries this device can manage', async () => {
  const core = await renderSection();

  expect(screen.getByText(copy.security.pairedHosts.title)).toBeOnTheScreen();
  expect(screen.getByText(copy.security.sshKeys.title)).toBeOnTheScreen();
  expect(screen.queryByText('我的设备')).toBeNull();
  expect(screen.queryByText('连接记录')).toBeNull();
  expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual([]);
});

it('ignores a Hub address left in storage', async () => {
  const kv = new MemoryKv();
  kv.setJSON('settings.hub.url', 'http://rs-hub:7300');
  const core = await renderSection(kv);

  expect(screen.getByText(copy.security.pairedHosts.title)).toBeOnTheScreen();
  expect(screen.queryByText('我的设备')).toBeNull();
  expect(screen.queryByText('连接记录')).toBeNull();
  expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual([]);
});
